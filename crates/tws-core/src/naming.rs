use crate::model::slugify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Tmux,
    Zellij,
}

impl BackendKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Zellij => "zellij",
        }
    }

    pub fn regular_prefix(self, collection_name: &str, thread_name: &str) -> String {
        let prefix = match self {
            Self::Tmux => "tws",
            Self::Zellij => "twz",
        };
        format!(
            "{prefix}_{}_{}",
            slugify(collection_name),
            slugify(thread_name)
        )
    }

    pub fn root_prefix(self, thread_name: &str) -> String {
        let prefix = match self {
            Self::Tmux => "twsr",
            Self::Zellij => "twzr",
        };
        format!("{prefix}_{}", slugify(thread_name))
    }

    pub fn regular_name(self, collection_name: &str, thread_name: &str, label: &str) -> String {
        format!(
            "{}_{}",
            self.regular_prefix(collection_name, thread_name),
            slugify(label)
        )
    }

    pub fn root_name(self, thread_name: &str, label: &str) -> String {
        format!("{}_{}", self.root_prefix(thread_name), slugify(label))
    }

    pub fn is_managed_name(self, name: &str) -> bool {
        match self {
            Self::Tmux => name.starts_with("tws_") || name.starts_with("twsr_"),
            Self::Zellij => name.starts_with("twz_") || name.starts_with("twzr_"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_names_keep_existing_format() {
        let backend = BackendKind::Tmux;
        assert_eq!(backend.regular_prefix("Work", "Task"), "tws_work_task");
        assert_eq!(backend.root_prefix("Scratch"), "twsr_scratch");
        assert!(backend.is_managed_name("tws_work_task_main"));
        assert!(!backend.is_managed_name("twz_work_task_main"));
    }

    #[test]
    fn zellij_names_are_isolated() {
        let backend = BackendKind::Zellij;
        assert_eq!(backend.regular_prefix("Work", "Task"), "twz_work_task");
        assert_eq!(backend.root_prefix("Scratch"), "twzr_scratch");
        assert!(backend.is_managed_name("twz_work_task_main"));
        assert!(!backend.is_managed_name("tws_work_task_main"));
    }
}
