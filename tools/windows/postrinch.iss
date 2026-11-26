; The Windows installer (DESIGN.md, "Installing and updating"): a per-user
; Setup that needs no administrator and puts Postrinch where its own updater
; keeps it current (%LOCALAPPDATA%\Programs\Postrinch, postrinch-update's
; install_dir), with the install marker, a Start-menu entry and an
; uninstaller in Apps & features.
;
; Built by .github/workflows/release.yml with Inno Setup 6:
;   iscc /DAppVersion=X.Y.Z /DStage=<folder with the three programs, the
;        marker, README.md and the licenses> /DOutDir=dist tools\windows\postrinch.iss

#ifndef AppVersion
  #error AppVersion is required (/DAppVersion=X.Y.Z)
#endif
#ifndef Stage
  #error Stage is required (/DStage=<folder>)
#endif
#ifndef OutDir
  #define OutDir "dist"
#endif

[Setup]
; Keep the AppId for every release: it is how Windows knows a new Setup is an
; upgrade of this app rather than another one.
AppId={{18A87A0C-8DA8-4CF8-98D3-AE6B5E169DF9}
AppName=Postrinch
AppVersion={#AppVersion}
AppVerName=Postrinch {#AppVersion}
AppPublisher=LBME Publishing
AppPublisherURL=https://postrinch.jkbase.app
AppSupportURL=https://github.com/joeleaver/postrinch/issues
AppCopyright=Copyright (C) 2026 LBME Publishing
; The folder the updater keeps current; per user, so no elevation.
DefaultDirName={localappdata}\Programs\Postrinch
DisableDirPage=yes
DefaultGroupName=Postrinch
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutDir}
OutputBaseFilename=postrinch-{#AppVersion}-windows-x86_64-setup
SetupIconFile=..\..\crates\postrinch\assets\icons\postrinch.ico
UninstallDisplayIcon={app}\postrinch.exe
UninstallDisplayName=Postrinch
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; A running Postrinch holds postrinch.exe open, and an agent's app may hold
; postrinch-mcp.exe: Setup offers to close them.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "Put Postrinch on the desktop"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#Stage}\postrinch.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\postrinch-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\postrinch-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
; The marker that makes this folder a managed install: the window updates
; itself from here on ("Restart to update").
Source: "{#Stage}\postrinch-install.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE-IBM-Plex.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Postrinch"; Filename: "{app}\postrinch.exe"
Name: "{userdesktop}\Postrinch"; Filename: "{app}\postrinch.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\postrinch.exe"; Description: "Open Postrinch"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; What the updater leaves beside the programs: a staged update, programs
; moved aside. Mail, settings and the keychain's sign-ins live elsewhere
; and are never touched.
Type: filesandordirs; Name: "{app}\update"
Type: files; Name: "{app}\*.old"
Type: files; Name: "{app}\*.incoming"
