; The Tulpar Control installer (Inno Setup 6). Build it with installer\build.ps1, which builds the app first and
; passes the version from Cargo.toml.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppName "Tulpar Control"
#define AppExe "TulparControl.exe"

[Setup]
AppId={{7B35BD08-3A02-4F7E-8F9F-A0355537E388}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Thnorty
AppPublisherURL=https://github.com/Thnorty/TulparControl
AppComments=Lighting, performance modes, fan curves and battery care for the Monster Tulpar T5 V21.4
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; The app needs administrator rights for the BIOS and the embedded controller, so it goes in Program Files.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir=..\dist
OutputBaseFilename=TulparControl-{#AppVersion}-Setup
SetupIconFile=..\res\app.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
; The running app is closed with "--exit" (see PrepareToInstall), which hands the Fn keys and fans back first.
CloseApplications=no
VersionInfoVersion={#AppVersion}
VersionInfoDescription={#AppName} setup

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start {#AppName} when I sign in (in the tray)"; GroupDescription: "When Windows starts:"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\FEATURES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\PLUGINS.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
; The same startup task as the app's own "Start with Windows" switch, so the switch shows it.
Filename: "{app}\{#AppExe}"; Parameters: "--enable-startup"; StatusMsg: "Setting {#AppName} to start with Windows..."; Flags: runhidden waituntilterminated; Tasks: autostart
Filename: "{app}\{#AppExe}"; Parameters: "--disable-startup"; Flags: runhidden waituntilterminated; Tasks: not autostart
Filename: "{app}\{#AppExe}"; Description: "Open {#AppName}"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{app}\{#AppExe}"; Parameters: "--exit"; RunOnceId: "Exit"; Flags: runhidden waituntilterminated
Filename: "{app}\{#AppExe}"; Parameters: "--disable-startup"; RunOnceId: "DisableStartup"; Flags: runhidden waituntilterminated

[Code]
// Before files are replaced: quit the running app (whichever copy it is) the way its Exit menu does, so the Fn keys
// and the fans are handed back to the laptop. The new exe is used, since an older one doesn't know --exit.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Result := '';
  ExtractTemporaryFile('{#AppExe}');
  if not Exec(ExpandConstant('{tmp}\{#AppExe}'), '--exit', '', SW_HIDE, ewWaitUntilTerminated, Code) or (Code <> 0) then
    Result := '{#AppName} is still running. Close it from its tray icon (Exit), then try again.';
end;
