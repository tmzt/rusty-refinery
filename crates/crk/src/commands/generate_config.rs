use std::path::PathBuf;

use librefinery::gen_config::{self, Editor, GenerateOptions};

pub fn run(
    editor: Editor,
    proxy: bool,
    socket: Option<String>,
    binary: Option<PathBuf>,
    planning_path: Option<String>,
    redis_url: Option<String>,
    allow_unsafe: bool,
    save: bool,
    replace_file: bool,
) {
    let binary_path = binary.unwrap_or_else(|| {
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("crk"))
    });
    let output = gen_config::generate(&GenerateOptions {
        editor: editor.clone(),
        binary_path,
        proxy,
        socket_path: socket,
        planning_path,
        redis_url,
        allow_unsafe,
    });
    if save {
        match gen_config::save(&editor, &output, replace_file) {
            Ok(path) => eprintln!("Wrote config to {}", path.display()),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        eprintln!("# Save to: {}", editor.config_path_hint());
        println!("{output}");
    }
}
