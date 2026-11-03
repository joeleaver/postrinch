//! The install's place on the desktop: on Linux, links on the PATH, the
//! icon in the hicolor theme and a menu entry. Elsewhere nothing yet.

use std::path::Path;

/// The icon, as the window crate carries it.
pub struct Icons {
    /// Square PNGs by edge length, for `hicolor/<n>x<n>/apps`.
    pub pngs: &'static [(u32, &'static [u8])],
    pub svg: &'static [u8],
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn integrate(_dir: &Path, _icons: &Icons) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn integrate(dir: &Path, icons: &Icons) -> Result<(), String> {
    let data = dirs::data_local_dir().ok_or("this system has no local data folder")?;
    if let Some(bin) = dirs::executable_dir() {
        link_programs(dir, &bin)?;
    }
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
    )
}

/// Link each program from `bin`, replacing a link an earlier install made,
/// never a file someone else put there.
#[cfg(target_os = "linux")]
fn link_programs(dir: &Path, bin: &Path) -> Result<(), String> {
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
            tracing::warn!(
                "{} is not a link Postrinch made, so it was left alone",
                link.display()
            );
            continue;
        }
        std::os::unix::fs::symlink(dir.join(name), &link)
            .map_err(|e| format!("linking {}: {e}", link.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn links_replace_links_and_leave_files_alone() {
        let root = tempfile::tempdir().unwrap();
        let (dir, bin) = (root.path().join("app"), root.path().join("bin"));
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink("/elsewhere/postrinch", bin.join("postrinch")).unwrap();
        std::fs::write(bin.join("postrinch-cli"), "someone's own").unwrap();
        link_programs(&dir, &bin).unwrap();
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
