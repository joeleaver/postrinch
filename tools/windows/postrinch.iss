; The Windows installer (DESIGN.md, "Installing and updating"): a per-user
; Setup that needs no administrator and puts Postrinch where its own updater
; keeps it current (%LOCALAPPDATA%\Programs\Postrinch, postrinch-update's
; install_dir), with the install marker, a Start-menu entry, the folder on
; the person's PATH and an uninstaller in Apps & features: what
; `postrinch.exe --install` from the zip leaves too (postrinch-update's
; win module), so the two routes end in the same install.
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
; The PATH changes ([Code]): Setup and its uninstaller tell running programs
; to read the environment again.
ChangesEnvironment=yes

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
; The entry `--install` makes the same (postrinch-update's win/shortcut.rs).
Name: "{userprograms}\Postrinch"; Filename: "{app}\postrinch.exe"; WorkingDir: "{app}"; Comment: "Your mail, on your computer, with agents you let in"
Name: "{userdesktop}\Postrinch"; Filename: "{app}\postrinch.exe"; WorkingDir: "{app}"; Comment: "Your mail, on your computer, with agents you let in"; Tasks: desktopicon

[Run]
Filename: "{app}\postrinch.exe"; Description: "Open Postrinch"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; What the updater leaves beside the programs: a staged update, programs
; moved aside. Mail, settings and the keychain's sign-ins live elsewhere
; and are never touched.
Type: filesandordirs; Name: "{app}\update"
Type: files; Name: "{app}\*.old"
Type: files; Name: "{app}\*.incoming"

[Code]
{ The folder on the person's own PATH (HKCU\Environment, Path), as
  postrinch-update's win/path.rs does it: added once, and recorded in
  HKCU\Software\Postrinch (PathAdded) so the uninstaller removes only an
  entry an install added, never one the person had. Entries compare case
  aside, a trailing backslash aside and environment variables expanded. }

const
  EnvKey = 'Environment';
  StateKey = 'Software\Postrinch';
  ZipEntryKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\Postrinch';

function ExpandEnvironmentStrings(Src: String; Dst: String; Size: DWORD): DWORD;
  external 'ExpandEnvironmentStringsW@kernel32.dll stdcall';

function Expanded(S: String): String;
var
  Buf: String;
  N: DWORD;
begin
  Result := S;
  if Pos('%', S) = 0 then exit;
  SetLength(Buf, 4096);
  N := ExpandEnvironmentStrings(S, Buf, 4096);
  if (N > 0) and (N <= 4096) then
    Result := Copy(Buf, 1, N - 1);
end;

function Normal(Entry: String): String;
begin
  Entry := Trim(Entry);
  StringChangeEx(Entry, '"', '', True);
  Entry := Expanded(Entry);
  StringChangeEx(Entry, '/', '\', True);
  while (Length(Entry) > 0) and (Entry[Length(Entry)] = '\') do
    Delete(Entry, Length(Entry), 1);
  Result := Lowercase(Entry);
end;

{ The person's PATH without the entries that are Dir, the rest as they were;
  Found says whether there were any. }
function Without(Path: String; Dir: String; var Found: Boolean): String;
var
  Rest, Entry, Want: String;
  P: Integer;
begin
  Want := Normal(Dir);
  Found := False;
  Result := '';
  Rest := Path;
  while Length(Rest) > 0 do begin
    P := Pos(';', Rest);
    if P = 0 then begin
      Entry := Rest;
      Rest := '';
    end else begin
      Entry := Copy(Rest, 1, P - 1);
      Delete(Rest, 1, P);
    end;
    if (Trim(Entry) <> '') and (Normal(Entry) = Want) then
      Found := True
    else if Result = '' then
      Result := Entry
    else
      Result := Result + ';' + Entry;
  end;
  while (Length(Result) > 0) and (Result[Length(Result)] = ';') do
    Delete(Result, Length(Result), 1);
end;

function ReadPath(var Path: String): Boolean;
begin
  Path := '';
  Result := RegQueryStringValue(HKCU, EnvKey, 'Path', Path);
end;

procedure WritePath(Path: String);
begin
  RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Path);
end;

procedure AddToPath(Dir: String);
var
  Path, Trimmed: String;
  Found: Boolean;
begin
  ReadPath(Path);
  Without(Path, Dir, Found);
  if Found then exit;
  Trimmed := Path;
  while (Length(Trimmed) > 0) and (Trimmed[Length(Trimmed)] = ';') do
    Delete(Trimmed, Length(Trimmed), 1);
  if Trim(Trimmed) = '' then
    WritePath(Dir)
  else
    WritePath(Trimmed + ';' + Dir);
  RegWriteStringValue(HKCU, StateKey, 'PathAdded', Dir);
end;

procedure RemoveFromPath;
var
  Added, Path, Left: String;
  Found: Boolean;
begin
  if not RegQueryStringValue(HKCU, StateKey, 'PathAdded', Added) then exit;
  if ReadPath(Path) then begin
    Left := Without(Path, Added, Found);
    if Found then WritePath(Left);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then begin
    AddToPath(ExpandConstant('{app}'));
    { Over an install from the zip: this Setup's entry in Apps & features
      stands for the folder from now on, not the zip install's own. }
    RegDeleteKeyIncludingSubkeys(HKCU, ZipEntryKey);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then begin
    RemoveFromPath;
    RegDeleteKeyIncludingSubkeys(HKCU, StateKey);
  end;
end;
