//! Linux: links on the PATH, the icon in the hicolor theme and a menu
//! entry.

use std::path::Path;

use super::Icons;

pub(super) fn integrate(dir: &Path, icons: &Icons) -> Result<Vec<String>, String> {
    let data = dirs::data_local_dir().ok_or("this system has no local data folder")?;
    let notes = match dirs::executable_dir() {
        Some(bin) => link_programs(dir, &bin)?,
        None => Vec::new(),
    };
    let hicolor = data.join("icons").join("hicolor");
    for (size, bytes) in icons.pngs {
        write(
            &hicolor
                .join(format!("{size}x{size}"))
                .join("apps")
                .join("postrinch.png"),
            bytes,
        )?;
    }
    write(
        &hicolor.join("scalable").join("apps").join("postrinch.svg"),
        icons.svg,
    )?;
    // The entry's file name matches the window's Wayland app id, so the
    // desktop puts the window under this launcher and its icon. It replaces
    // the hidden stub rinch writes for that id.
    write(
        &data.join("applications").join("postrinch.desktop"),
        entry(dir).as_bytes(),
    )?;
    Ok(notes)
}

/// Link each program from `bin`, replacing a link an earlier install made,
/// never a file someone else put there. Answers a sentence for each one
/// left alone.
fn link_programs(dir: &Path, bin: &Path) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    std::fs::create_dir_all(bin).map_err(|e| format!("creating {}: {e}", bin.display()))?;
    for name in crate::PROGRAMS {
        let link = bin.join(name);
        let is_link = link
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink());
        if is_link {
            std::fs::remove_file(&link)
                .map_err(|e| format!("replacing {}: {e}", link.display()))?;
        }
        if link.symlink_metadata().is_ok() {
            notes.push(format!(
                "{} is not a link Postrinch made, so it was left alone",
                link.display()
            ));
            continue;
        }
        std::os::unix::fs::symlink(dir.join(name), &link)
            .map_err(|e| format!("linking {}: {e}", link.display()))?;
    }
    Ok(notes)
}

fn entry(dir: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Postrinch\n\
         GenericName=Mail\n\
         Comment=Your mail, on your computer, with agents you let in\n\
         Exec=\"{}\"\n\
         Icon=postrinch\n\
         Terminal=false\n\
         Categories=Network;Email;\n\
         Keywords=mail;email;gmail;\n\
         StartupWMClass=postrinch\n",
        dir.join("postrinch").display()
    )
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_replace_links_and_leave_files_alone() {
        let root = tempfile::tempdir().unwrap();
        let (dir, bin) = (root.path().join("app"), root.path().join("bin"));
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink("/elsewhere/postrinch", bin.join("postrinch")).unwrap();
        std::fs::write(bin.join("postrinch-cli"), "someone's own").unwrap();
        let notes = link_programs(&dir, &bin).unwrap();
        assert_eq!(
            notes,
            [format!(
                "{} is not a link Postrinch made, so it was left alone",
                bin.join("postrinch-cli").display()
            )]
        );
        assert_eq!(
            std::fs::read_link(bin.join("postrinch")).unwrap(),
            dir.join("postrinch")
        );
        assert_eq!(
            std::fs::read_link(bin.join("postrinch-mcp")).unwrap(),
            dir.join("postrinch-mcp")
        );
        assert_eq!(
            std::fs::read_to_string(bin.join("postrinch-cli")).unwrap(),
            "someone's own"
        );
    }

    #[test]
    fn the_entry_starts_the_installed_window() {
        let text = entry(Path::new("/home/a b/.local/share/postrinch-app"));
        assert!(
            text.contains("Exec=\"/home/a b/.local/share/postrinch-app/postrinch\"\n"),
            "{text}"
        );
        assert!(text.contains("StartupWMClass=postrinch\n"));
        assert!(!text.contains("X-Rinch-Stub"));
    }
}
