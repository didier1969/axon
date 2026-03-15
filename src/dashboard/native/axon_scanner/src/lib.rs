use rustler::{Atom, Encoder, LocalPid, NifResult, OwnedEnv};
use ignore::WalkBuilder;
use std::path::Path;
use std::collections::HashSet;
use std::thread;
use walkdir::WalkDir;

mod atoms {
    rustler::atoms! {
        ok,
    }
}

#[rustler::nif]
fn scan(path: String) -> NifResult<Vec<String>> {
    let root_path = Path::new(&path);
    let mut files_set = HashSet::new();
    
    // 1. Scan standard respectant .axonignore
    let mut builder = WalkBuilder::new(root_path);
    builder.git_ignore(false);
    builder.git_global(false);
    builder.git_exclude(false);
    builder.add_custom_ignore_filename(".axonignore");
    
    // Ajout des filtres globaux
    let agence_ignore = Path::new("/home/dstadel/projects/.axonignore");
    if agence_ignore.exists() { builder.add_ignore(agence_ignore); }
    let moteur_ignore = Path::new("/home/dstadel/projects/axon/.axonignore");
    if moteur_ignore.exists() { builder.add_ignore(moteur_ignore); }

    for result in builder.build() {
        if let Ok(entry) = result {
            if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                files_set.insert(entry.path().to_string_lossy().into_owned());
            }
        }
    }
    
    // 2. Règle d'Or : Scan forcé de TOUS les .md (même dans dossiers ignorés)
    for entry in WalkDir::new(root_path)
        .into_iter()
        .filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                if entry.path().extension().map(|ext| ext == "md").unwrap_or(false) {
                    files_set.insert(entry.path().to_string_lossy().into_owned());
                }
            }
        }
    
    let mut final_list: Vec<String> = files_set.into_iter().collect();
    final_list.sort();
    
    Ok(final_list)
}

#[rustler::nif]
fn start_streaming(path: String, pid: LocalPid) -> NifResult<Atom> {
    thread::spawn(move || {
        let mut owned_env = OwnedEnv::new();
        let root_path = Path::new(&path);
        let mut sent_files = HashSet::new();

        // 1. Scan standard respectant .axonignore
        let mut builder = WalkBuilder::new(root_path);
        builder.git_ignore(false);
        builder.git_global(false);
        builder.git_exclude(false);
        builder.add_custom_ignore_filename(".axonignore");

        let agence_ignore = Path::new("/home/dstadel/projects/.axonignore");
        if agence_ignore.exists() { builder.add_ignore(agence_ignore); }
        let moteur_ignore = Path::new("/home/dstadel/projects/axon/.axonignore");
        if moteur_ignore.exists() { builder.add_ignore(moteur_ignore); }

        for result in builder.build() {
            if let Ok(entry) = result {
                if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
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

        // 2. Règle d'Or : Scan forcé de TOUS les .md (même dans dossiers ignorés)
        for entry in WalkDir::new(root_path)
            .into_iter()
            .filter_map(|e| e.ok()) {
                if entry.file_type().is_file() {
                    if entry.path().extension().map(|ext| ext == "md").unwrap_or(false) {
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

        // Send "done" message
        let _ = owned_env.send_and_clear(&pid, |env| {
            (atoms::ok(), "done").encode(env)
        });
    });

    Ok(atoms::ok())
}

rustler::init!("Elixir.Axon.Scanner");
