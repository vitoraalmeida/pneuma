/// Interface-neutral command vocabulary. The CLI maps parsed arguments onto
/// these commands; later adapters issue the same commands without Clap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SystemCreate {
        name: String,
        description: Option<String>,
    },
    SystemList,
    SystemShow {
        name: String,
    },
}
