use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::Args;
use serde::Serialize;

use crate::core::model::{Collection, Thread, slugify};
use crate::core::persistence;
use tws_mux as mux;

#[derive(Args)]
pub struct EnsureCollectionArgs {
    /// Collection name. Exact matches are reused.
    pub name: String,

    /// Print a machine-readable result.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct EnsureThreadArgs {
    /// Existing collection name.
    #[arg(long)]
    pub collection: String,

    /// Thread name. Exact matches are reused.
    pub name: String,

    /// Print a machine-readable result.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
#[command(trailing_var_arg = true)]
pub struct SpawnArgs {
    /// Collection containing the thread.
    #[arg(long)]
    pub collection: String,

    /// Thread receiving the session.
    #[arg(long)]
    pub thread: String,

    /// Session label shown below the thread.
    #[arg(long)]
    pub label: String,

    /// Initial working directory.
    #[arg(long)]
    pub cwd: PathBuf,

    /// Create a missing collection and thread before spawning.
    #[arg(long)]
    pub ensure_hierarchy: bool,

    /// Return an existing session instead of failing. The command is not run again.
    #[arg(long)]
    pub reuse: bool,

    /// Print a machine-readable result.
    #[arg(long)]
    pub json: bool,

    /// Optional command to run, supplied after `--`.
    #[arg(value_name = "COMMAND")]
    pub command: Vec<OsString>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectionResult {
    created: bool,
    name: String,
    id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadResult {
    created: bool,
    collection: String,
    name: String,
    id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnResult {
    created_collection: bool,
    created_thread: bool,
    created_session: bool,
    command_started: bool,
    session_name: String,
    backend: String,
    cwd: String,
}

struct HierarchyResult {
    collections: Vec<Collection>,
    col_idx: usize,
    thread_idx: Option<usize>,
    created_collection: bool,
    created_thread: bool,
}

pub fn ensure_collection(args: EnsureCollectionArgs) -> Result<(), String> {
    let name = clean_name("collection", &args.name)?;
    let hierarchy = ensure_hierarchy(&name, None, true)?;
    let collection = &hierarchy.collections[hierarchy.col_idx];
    let result = CollectionResult {
        created: hierarchy.created_collection,
        name: collection.name.clone(),
        id: collection.id.to_string(),
    };
    print_result(
        args.json,
        &result,
        if hierarchy.created_collection {
            format!("Created collection {:?}.", collection.name)
        } else {
            format!("Collection {:?} already exists.", collection.name)
        },
    )
}

pub fn ensure_thread(args: EnsureThreadArgs) -> Result<(), String> {
    let collection_name = clean_name("collection", &args.collection)?;
    let thread_name = clean_name("thread", &args.name)?;
    let hierarchy = ensure_hierarchy(&collection_name, Some(&thread_name), false)?;
    let thread_idx = hierarchy
        .thread_idx
        .ok_or_else(|| "thread was not created".to_string())?;
    let thread = &hierarchy.collections[hierarchy.col_idx].threads[thread_idx];
    let result = ThreadResult {
        created: hierarchy.created_thread,
        collection: hierarchy.collections[hierarchy.col_idx].name.clone(),
        name: thread.name.clone(),
        id: thread.id.to_string(),
    };
    print_result(
        args.json,
        &result,
        if hierarchy.created_thread {
            format!(
                "Created thread {:?} in collection {:?}.",
                thread.name, hierarchy.collections[hierarchy.col_idx].name
            )
        } else {
            format!(
                "Thread {:?} already exists in collection {:?}.",
                thread.name, hierarchy.collections[hierarchy.col_idx].name
            )
        },
    )
}

pub fn spawn(args: SpawnArgs) -> Result<(), String> {
    if !args.command.is_empty() && mux::backend() == mux::Backend::Zellij {
        return Err(
            "starting a command with `tws spawn` is not supported by the Zellij backend"
                .to_string(),
        );
    }

    let collection_name = clean_name("collection", &args.collection)?;
    let thread_name = clean_name("thread", &args.thread)?;
    let label = clean_name("session label", &args.label)?;
    if slugify(&label).is_empty() {
        return Err("session label must contain at least one letter or number".to_string());
    }
    let cwd = canonical_directory(&args.cwd)?;

    let (collections, created_collection, created_thread, col_idx, thread_idx) = if args
        .ensure_hierarchy
    {
        let hierarchy = ensure_hierarchy(&collection_name, Some(&thread_name), true)?;
        let thread_idx = hierarchy
            .thread_idx
            .ok_or_else(|| "thread was not created".to_string())?;
        (
            hierarchy.collections,
            hierarchy.created_collection,
            hierarchy.created_thread,
            hierarchy.col_idx,
            thread_idx,
        )
    } else {
        let collections = persistence::load().map_err(|error| error.to_string())?;
        let col_idx = find_collection(&collections, &collection_name)?.ok_or_else(|| {
            format!(
                "collection {:?} does not exist; create it first or pass --ensure-hierarchy",
                collection_name
            )
        })?;
        let thread_idx = find_thread(&collections[col_idx], &thread_name)?.ok_or_else(|| {
                format!(
                    "thread {:?} does not exist in collection {:?}; create it first or pass --ensure-hierarchy",
                    thread_name, collection_name
                )
            })?;
        (collections, false, false, col_idx, thread_idx)
    };

    let session_name = mux::regular_name(
        &collections[col_idx].name,
        &collections[col_idx].threads[thread_idx].name,
        &label,
    );
    let existing = mux::list_sessions()?
        .into_iter()
        .any(|name| name == session_name);
    if existing {
        if !args.reuse {
            return Err(format!(
                "session {:?} already exists; pass --reuse to return it unchanged",
                session_name
            ));
        }
        return print_spawn_result(
            args.json,
            SpawnResult {
                created_collection,
                created_thread,
                created_session: false,
                command_started: false,
                session_name,
                backend: mux::name().to_string(),
                cwd: cwd.display().to_string(),
            },
        );
    }

    if args.command.is_empty() {
        mux::new_session_in_dir(&session_name, &cwd)?;
    } else {
        mux::new_session_in_dir_with_command(&session_name, &cwd, &args.command)?;
    }

    if !mux::list_sessions()?
        .into_iter()
        .any(|name| name == session_name)
    {
        return Err(format!(
            "session {:?} was created but is no longer running; the startup command may have exited immediately",
            session_name
        ));
    }

    print_spawn_result(
        args.json,
        SpawnResult {
            created_collection,
            created_thread,
            created_session: true,
            command_started: !args.command.is_empty(),
            session_name,
            backend: mux::name().to_string(),
            cwd: cwd.display().to_string(),
        },
    )
}

fn print_spawn_result(json: bool, result: SpawnResult) -> Result<(), String> {
    let message = if result.created_session {
        format!(
            "Created {} session {:?}.",
            result.backend, result.session_name
        )
    } else {
        format!(
            "Reusing {} session {:?}.",
            result.backend, result.session_name
        )
    };
    print_result(json, &result, message)
}

fn print_result<T: Serialize>(json: bool, result: &T, message: String) -> Result<(), String> {
    if json {
        println!(
            "{}",
            serde_json::to_string(result).map_err(|error| error.to_string())?
        );
    } else {
        println!("{message}");
    }
    Ok(())
}

/// Ensure a collection and, optionally, a thread. Returns whether the requested
/// leaf item was created. Existing exact names are idempotent; names that only
/// collide after slugification are rejected because their sessions would be
/// indistinguishable to tws.
fn ensure_hierarchy(
    collection_name: &str,
    thread_name: Option<&str>,
    create_collection: bool,
) -> Result<HierarchyResult, String> {
    let current = persistence::load().map_err(|error| error.to_string())?;
    let current_col = find_collection(&current, collection_name)?;
    let current_thread = match (current_col, thread_name) {
        (Some(col_idx), Some(name)) => find_thread(&current[col_idx], name)?,
        _ => None,
    };
    let already_exists =
        current_col.is_some() && (thread_name.is_none() || current_thread.is_some());
    if already_exists {
        return Ok(HierarchyResult {
            collections: current,
            col_idx: current_col.expect("existing collection"),
            thread_idx: current_thread,
            created_collection: false,
            created_thread: false,
        });
    }

    let _lock = acquire_mutation_lock()?;
    let mut collections = persistence::load().map_err(|error| error.to_string())?;
    let mut created_collection = false;
    let mut created_thread = false;
    let col_idx = match find_collection(&collections, collection_name)? {
        Some(idx) => idx,
        None if create_collection => {
            reject_collection_slug_collision(&collections, collection_name)?;
            collections.push(Collection::new(collection_name));
            created_collection = true;
            collections.len() - 1
        }
        None => {
            return Err(format!(
                "collection {:?} does not exist; create it first with `tws collection ensure`",
                collection_name
            ));
        }
    };

    let thread_idx = if let Some(name) = thread_name {
        match find_thread(&collections[col_idx], name)? {
            Some(idx) => Some(idx),
            None => {
                reject_thread_slug_collision(&collections[col_idx], name)?;
                collections[col_idx].threads.push(Thread::new(name));
                created_thread = true;
                Some(collections[col_idx].threads.len() - 1)
            }
        }
    } else {
        None
    };

    if created_collection || created_thread {
        persistence::save(&collections).map_err(|error| error.to_string())?;
    }
    Ok(HierarchyResult {
        collections,
        col_idx,
        thread_idx,
        created_collection,
        created_thread,
    })
}

fn acquire_mutation_lock() -> Result<persistence::LockGuard, String> {
    match persistence::acquire_instance_lock() {
        persistence::LockState::Acquired(guard) => Ok(guard),
        persistence::LockState::HeldByOther(pid) => Err(format!(
            "state is locked by a running tws instance (pid {pid}); create the hierarchy in the TUI or close it and retry"
        )),
        persistence::LockState::Failed(error) => {
            Err(format!("could not acquire the state lock: {error}"))
        }
    }
}

fn find_collection(collections: &[Collection], name: &str) -> Result<Option<usize>, String> {
    let matches: Vec<usize> = collections
        .iter()
        .enumerate()
        .filter(|(_, collection)| !collection.is_root && collection.name == name)
        .map(|(idx, _)| idx)
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [idx] => Ok(Some(*idx)),
        _ => Err(format!(
            "multiple collections named {:?} exist; resolve the ambiguity in the TUI first",
            name
        )),
    }
}

fn find_thread(collection: &Collection, name: &str) -> Result<Option<usize>, String> {
    let matches: Vec<usize> = collection
        .threads
        .iter()
        .enumerate()
        .filter(|(_, thread)| thread.name == name)
        .map(|(idx, _)| idx)
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [idx] => Ok(Some(*idx)),
        _ => Err(format!(
            "multiple threads named {:?} exist in collection {:?}; resolve the ambiguity in the TUI first",
            name, collection.name
        )),
    }
}

fn reject_collection_slug_collision(
    collections: &[Collection],
    requested: &str,
) -> Result<(), String> {
    let slug = slugify(requested);
    if let Some(existing) = collections
        .iter()
        .find(|collection| !collection.is_root && slugify(&collection.name) == slug)
    {
        return Err(format!(
            "collection {:?} collides with existing collection {:?} after slugification ({:?})",
            requested, existing.name, slug
        ));
    }
    Ok(())
}

fn reject_thread_slug_collision(collection: &Collection, requested: &str) -> Result<(), String> {
    let slug = slugify(requested);
    if let Some(existing) = collection
        .threads
        .iter()
        .find(|thread| slugify(&thread.name) == slug)
    {
        return Err(format!(
            "thread {:?} collides with existing thread {:?} after slugification ({:?})",
            requested, existing.name, slug
        ));
    }
    Ok(())
}

fn clean_name(kind: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("{kind} name must not be empty"))
    } else if slugify(value).is_empty() {
        Err(format!(
            "{kind} name must contain at least one letter or number"
        ))
    } else {
        Ok(value.to_string())
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("cannot use working directory {}: {error}", path.display()))?;
    if !path.is_dir() {
        return Err(format!(
            "working directory {} is not a directory",
            path.display()
        ));
    }
    Ok(path)
}
