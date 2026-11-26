use super::*;
use tempfile::TempDir;

/// The shim npm's `cmd-shim` writes for a JavaScript program.
const JS_SHIM: &str = r#"@ECHO off
GOTO start
:find_dp0
SET dp0=%~dp0
EXIT /b
:start
SETLOCAL
CALL :find_dp0

IF EXIST "%dp0%\node.exe" (
  SET "_prog=%dp0%\node.exe"
) ELSE (
  SET "_prog=node"
  SET PATHEXT=%PATHEXT:;.JS;=;%
)

endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & "%_prog%"  "%dp0%\node_modules\@anthropic-ai\claude-code\cli.js" %*
"#;

/// The shim `cmd-shim` writes for a program that is not JavaScript.
const EXE_SHIM: &str = r#"@ECHO off
GOTO start
:find_dp0
SET dp0=%~dp0
EXIT /b
:start
SETLOCAL
CALL :find_dp0
"%dp0%\node_modules\@anthropic-ai\claude-code\bin\claude.exe"   %*
"#;

fn touch(path: &Path) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "").unwrap();
    path.to_path_buf()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Places with only `path` on the PATH and nothing else to fall back on.
fn on_path(path: &[&Path]) -> Places {
    Places {
        path: Some(std::env::join_paths(path).unwrap()),
        home: None,
        app_data: None,
    }
}

fn cli_js(npm: &Path) -> PathBuf {
    npm.join("node_modules")
        .join("@anthropic-ai")
        .join("claude-code")
        .join("cli.js")
}

#[test]
fn a_native_exe_on_the_path_is_started_as_it_is() {
    let dir = TempDir::new().unwrap();
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let exe = touch(&dir.path().join("bin").join("claude.exe"));
    let found = find_in(&on_path(&[&empty, &dir.path().join("bin")]), Os::Windows);
    assert_eq!(
        found,
        Some(Launch {
            program: exe,
            args: Vec::new()
        })
    );
}

#[test]
fn an_npm_shim_runs_its_script_with_the_node_beside_it() {
    let dir = TempDir::new().unwrap();
    let npm = dir.path().join("npm");
    write(&npm.join("claude.cmd"), JS_SHIM);
    let script = touch(&cli_js(&npm));
    let node = touch(&npm.join("node.exe"));
    let found = find_in(&on_path(&[&npm]), Os::Windows).unwrap();
    assert_eq!(found.program, node);
    assert_eq!(found.args, vec![script.into_os_string()]);
}

#[test]
fn an_npm_shim_runs_its_script_with_the_node_on_the_path() {
    let dir = TempDir::new().unwrap();
    let npm = dir.path().join("npm");
    let nodejs = dir.path().join("nodejs");
    write(&npm.join("claude.cmd"), JS_SHIM);
    let script = touch(&cli_js(&npm));
    let node = touch(&nodejs.join("node.exe"));
    let found = find_in(&on_path(&[&npm, &nodejs]), Os::Windows).unwrap();
    assert_eq!(found.program, node);
    assert_eq!(found.args, vec![script.into_os_string()]);
}

#[test]
fn an_npm_shim_without_node_is_not_claude_code() {
    let dir = TempDir::new().unwrap();
    let npm = dir.path().join("npm");
    write(&npm.join("claude.cmd"), JS_SHIM);
    touch(&cli_js(&npm));
    assert_eq!(find_in(&on_path(&[&npm]), Os::Windows), None);
}

#[test]
fn an_npm_shim_whose_script_is_gone_is_not_claude_code() {
    let dir = TempDir::new().unwrap();
    let npm = dir.path().join("npm");
    write(&npm.join("claude.cmd"), JS_SHIM);
    touch(&npm.join("node.exe"));
    assert_eq!(find_in(&on_path(&[&npm]), Os::Windows), None);
}

#[test]
fn an_npm_shim_of_a_program_starts_that_program() {
    let dir = TempDir::new().unwrap();
    let npm = dir.path().join("npm");
    write(&npm.join("claude.cmd"), EXE_SHIM);
    let exe = touch(
        &npm.join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code")
            .join("bin")
            .join("claude.exe"),
    );
    let found = find_in(&on_path(&[&npm]), Os::Windows).unwrap();
    assert_eq!(
        found,
        Launch {
            program: exe,
            args: Vec::new()
        }
    );
}

#[test]
fn the_first_folder_on_the_path_wins() {
    let dir = TempDir::new().unwrap();
    let npm = dir.path().join("npm");
    write(&npm.join("claude.cmd"), JS_SHIM);
    touch(&cli_js(&npm));
    let node = touch(&npm.join("node.exe"));
    touch(&dir.path().join("later").join("claude.exe"));
    let found = find_in(&on_path(&[&npm, &dir.path().join("later")]), Os::Windows).unwrap();
    assert_eq!(found.program, node);
}

#[test]
fn the_installers_folders_are_found_off_the_path() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("home");
    let exe = touch(&home.join(".local").join("bin").join("claude.exe"));
    let places = Places {
        path: None,
        home: Some(home),
        app_data: None,
    };
    assert_eq!(find_in(&places, Os::Windows).unwrap().program, exe);

    let app_data = dir.path().join("AppData");
    let npm = app_data.join("npm");
    write(&npm.join("claude.cmd"), JS_SHIM);
    let script = touch(&cli_js(&npm));
    let node = touch(&npm.join("node.exe"));
    let places = Places {
        path: Some(OsString::new()),
        home: None,
        app_data: Some(app_data),
    };
    let found = find_in(&places, Os::Windows).unwrap();
    assert_eq!((found.program, found.args), (node, vec![script.into()]));
}

#[test]
fn nothing_anywhere_is_not_found() {
    let dir = TempDir::new().unwrap();
    let bin = dir.path().join("bin");
    touch(&bin.join("claude"));
    touch(&bin.join("claude.ps1"));
    let places = Places {
        path: Some(std::env::join_paths([&bin]).unwrap()),
        home: Some(dir.path().join("home")),
        app_data: Some(dir.path().join("AppData")),
    };
    assert_eq!(find_in(&places, Os::Windows), None);
    let places = Places {
        path: None,
        home: None,
        app_data: None,
    };
    assert_eq!(find_in(&places, Os::Windows), None);
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn program(path: &Path) -> PathBuf {
        touch(path);
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_path_buf()
    }

    #[test]
    fn claude_on_the_path_is_started_as_it_is() {
        let dir = TempDir::new().unwrap();
        let claude = program(&dir.path().join("bin").join("claude"));
        let found = find_in(&on_path(&[&dir.path().join("bin")]), Os::Unix);
        assert_eq!(
            found,
            Some(Launch {
                program: claude,
                args: Vec::new()
            })
        );
    }

    #[test]
    fn a_file_that_cannot_run_is_passed_over() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join("first").join("claude"));
        let claude = program(&dir.path().join("second").join("claude"));
        let path = [&dir.path().join("first"), &dir.path().join("second")];
        let found = find_in(&on_path(&path.map(PathBuf::as_path)), Os::Unix).unwrap();
        assert_eq!(found.program, claude);
    }

    #[test]
    fn the_installers_folders_are_found_off_the_path() {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home");
        let claude = program(&home.join(".claude").join("local").join("claude"));
        let places = Places {
            path: Some(OsString::new()),
            home: Some(home),
            app_data: None,
        };
        assert_eq!(find_in(&places, Os::Unix).unwrap().program, claude);
    }

    #[test]
    fn nothing_is_not_found() {
        let dir = TempDir::new().unwrap();
        let places = Places {
            path: Some(std::env::join_paths([dir.path()]).unwrap()),
            home: Some(dir.path().to_path_buf()),
            app_data: None,
        };
        assert_eq!(find_in(&places, Os::Unix), None);
    }
}
