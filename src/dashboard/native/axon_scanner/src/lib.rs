use rustler::{Atom, Encoder, LocalPid, NifResult, OwnedEnv};
use ignore::WalkBuilder;
use std::path::Path;
use std::collections::HashSet;
use std::thread;

mod atoms {
    rustler::atoms! {
        ok,
    }
}

fn is_supported(path: &Path, extensions: &[String]) -> bool {
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        extensions.iter().any(|e| e.to_lowercase() == ext_str)
    } else {
        false
    }
}

#[rustler::nif]
fn scan(path: String, extensions: Vec<String>) -> NifResult<Vec<String>> {
    let root_path = Path::new(&path);
    let mut files_set = HashSet::new();
    
    // 1. Scan standard respectant .axonignore
    let mut builder = WalkBuilder::new(root_path);
    builder.hidden(false);
    builder.git_ignore(false);
    builder.git_global(false);
    builder.git_exclude(false);
    builder.add_custom_ignore_filename(".axonignore");
    builder.add_custom_ignore_filename(".axonignore.local");

    for result in builder.build() {
        if let Ok(entry) = result {
            if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                if is_supported(entry.path(), &extensions) {
                    files_set.insert(entry.path().to_string_lossy().into_owned());
                }
            }
        }
    }
    
    let mut final_list: Vec<String> = files_set.into_iter().collect();
    final_list.sort();
    
    Ok(final_list)
}

#[rustler::nif]
fn start_streaming(path: String, pid: LocalPid, extensions: Vec<String>) -> NifResult<Atom> {
    thread::spawn(move || {
        let mut owned_env = OwnedEnv::new();
        let root_path = Path::new(&path);
        let mut sent_files = HashSet::new();

        // Scan standard respectant .axonignore + Extension filtering
        let mut builder = WalkBuilder::new(root_path);
        builder.hidden(false);
        builder.git_ignore(false);
        builder.git_global(false);
        builder.git_exclude(false);
        builder.add_custom_ignore_filename(".axonignore");
        builder.add_custom_ignore_filename(".axonignore.local");

        for result in builder.build() {
            if let Ok(entry) = result {
                if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                    if is_supported(entry.path(), &extensions) {
                        let file_path = entry.path().to_string_lossy().into_owned();
                        if !sent_files.contains(&file_path) {
                            let _ = owned_env.send_and_clear(&pid, |env| {
                                (atoms::ok(), &file_path).encode(env)
                            });
                            sent_files.insert(file_path);
                        }
                    }
                }
            }
        }

        // Send "done" message
        let _ = owned_env.send_and_clear(&pid, |env| {
            (atoms::ok(), "done").encode(env)
        });
    });

    Ok(atoms::ok())
}

rustler::init!("Elixir.Axon.Scanner");
