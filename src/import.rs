use std::io::{self, BufRead, Write};

use crate::core::model::{Collection, Thread};
use crate::core::persistence;
use tws_mux as mux;

pub fn run() -> io::Result<()> {
    let all_sessions = mux::list_sessions().map_err(io::Error::other)?;
    let unmanaged: Vec<&String> = all_sessions
        .iter()
        .filter(|name| !mux::is_managed_name(name))
        .collect();

    if unmanaged.is_empty() {
        println!("No unmanaged {} sessions found.", mux::name());
        return Ok(());
    }

    println!(
        "Found {} unmanaged session(s): {}\n",
        unmanaged.len(),
        unmanaged
            .iter()
            .map(|s| format!("\"{}\"", s))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut collections = persistence::load()?;
    let mut modified = false;

    for session_name in &unmanaged {
        println!("── Session: \"{}\" ──", session_name);

        // Track what this iteration created so aborted imports don't leave
        // orphan collections/threads behind.
        let mut created_col = false;
        let mut created_thread_in: Option<usize> = None;
        fn rollback(
            collections: &mut Vec<Collection>,
            created_col: bool,
            created_thread_in: Option<usize>,
        ) {
            if let Some(ci) = created_thread_in {
                collections[ci].threads.pop();
            }
            if created_col {
                collections.pop();
            }
        }

        let col_idx = match pick_collection(&collections)? {
            Some(idx) => idx,
            None => {
                println!("Skipping \"{}\".\n", session_name);
                continue;
            }
        };

        // If pick_collection returned an index beyond current length, a new one was created
        if col_idx >= collections.len() {
            let name = prompt("  New collection name: ")?.unwrap_or_default();
            if name.is_empty() {
                println!("Skipping \"{}\".\n", session_name);
                continue;
            }
            collections.push(Collection::new(&name));
            created_col = true;
        }

        let thread_idx = match pick_thread(&collections[col_idx])? {
            Some(idx) => idx,
            None => {
                println!("Skipping \"{}\".\n", session_name);
                rollback(&mut collections, created_col, created_thread_in);
                continue;
            }
        };

        if thread_idx >= collections[col_idx].threads.len() {
            let name = prompt("  New thread name: ")?.unwrap_or_default();
            if name.is_empty() {
                println!("Skipping \"{}\".\n", session_name);
                rollback(&mut collections, created_col, created_thread_in);
                continue;
            }
            collections[col_idx].threads.push(Thread::new(&name));
            created_thread_in = Some(col_idx);
        }

        let label = prompt_label()?.unwrap_or_default();
        if label.is_empty() {
            println!("Skipping \"{}\".\n", session_name);
            rollback(&mut collections, created_col, created_thread_in);
            continue;
        }

        let col_name = &collections[col_idx].name;
        let thread_name = &collections[col_idx].threads[thread_idx].name;
        let new_name = mux::regular_name(col_name, thread_name, &label);

        println!("\n  Rename: \"{}\" → \"{}\"\n", session_name, new_name);

        if confirm("  Proceed?")? {
            match mux::rename_session(session_name, &new_name) {
                Ok(()) => {
                    println!("  Renamed successfully.\n");
                    if created_col || created_thread_in.is_some() {
                        modified = true;
                    }
                }
                Err(e) => {
                    println!("  Error: {}\n", e);
                    rollback(&mut collections, created_col, created_thread_in);
                }
            }
        } else {
            println!("  Skipped.\n");
            rollback(&mut collections, created_col, created_thread_in);
        }
    }

    if modified {
        persistence::save(&collections)?;
        println!("State saved.");
    }

    println!("Import complete.");
    Ok(())
}

fn pick_collection(collections: &[Collection]) -> io::Result<Option<usize>> {
    println!("  Select a collection:");
    for (i, col) in collections.iter().enumerate() {
        println!("    [{}] {}", i + 1, col.name);
    }
    let new_idx = collections.len() + 1;
    println!("    [{}] Create new collection", new_idx);
    println!("    [s] Skip this session");

    loop {
        let Some(input) = prompt("  Choice: ")? else {
            return Ok(None);
        };
        if input == "s" {
            return Ok(None);
        }
        if let Ok(n) = input.parse::<usize>() {
            if n >= 1 && n <= collections.len() {
                return Ok(Some(n - 1));
            }
            if n == new_idx {
                return Ok(Some(collections.len()));
            }
        }
        println!("  Invalid choice, try again.");
    }
}

fn pick_thread(collection: &Collection) -> io::Result<Option<usize>> {
    println!("  Select a thread in \"{}\":", collection.name);
    for (i, thread) in collection.threads.iter().enumerate() {
        println!("    [{}] {}", i + 1, thread.name);
    }
    let new_idx = collection.threads.len() + 1;
    println!("    [{}] Create new thread", new_idx);
    println!("    [s] Skip this session");

    loop {
        let Some(input) = prompt("  Choice: ")? else {
            return Ok(None);
        };
        if input == "s" {
            return Ok(None);
        }
        if let Ok(n) = input.parse::<usize>() {
            if n >= 1 && n <= collection.threads.len() {
                return Ok(Some(n - 1));
            }
            if n == new_idx {
                return Ok(Some(collection.threads.len()));
            }
        }
        println!("  Invalid choice, try again.");
    }
}

fn prompt_label() -> io::Result<Option<String>> {
    prompt("  Session label (e.g., main, debug): ")
}

fn prompt(msg: &str) -> io::Result<Option<String>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    prompt_from(&mut stdin.lock(), &mut stdout.lock(), msg)
}

fn prompt_from(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    msg: &str,
) -> io::Result<Option<String>> {
    write!(writer, "{}", msg)?;
    writer.flush()?;
    let mut input = String::new();
    if reader.read_line(&mut input)? == 0 {
        return Ok(None);
    }
    Ok(Some(input.trim().to_string()))
}

fn confirm(msg: &str) -> io::Result<bool> {
    let input = prompt(&format!("{} [y/N] ", msg))?;
    Ok(input.is_some_and(|input| input == "y" || input == "Y"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn prompt_returns_none_at_end_of_input() {
        let mut reader = Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();

        assert_eq!(
            prompt_from(&mut reader, &mut output, "Choice: ").unwrap(),
            None
        );
        assert_eq!(output, b"Choice: ");
    }

    #[test]
    fn prompt_trims_input() {
        let mut reader = Cursor::new(b"  main  \n".to_vec());
        let mut output = Vec::new();

        assert_eq!(
            prompt_from(&mut reader, &mut output, "Label: ").unwrap(),
            Some("main".to_string())
        );
    }
}
