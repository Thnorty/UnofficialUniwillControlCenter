; The Unofficial Uniwill Control Center installer (Inno Setup 6). Build it with installer\build.ps1, which builds the app first and
; passes the version from Cargo.toml.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppName "Unofficial Uniwill Control Center"
#define AppExe "UnofficialUniwillControlCenter.exe"
#define AppGuid "{7B35BD08-3A02-4F7E-8F9F-A0355537E388}"
; The app's single-instance lock (src/shell/instance.rs), named when it was called Tulpar Control.
#define AppLock "Local\TulparControl"

[Setup]
AppId={{#AppGuid}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Thnorty
AppPublisherURL=https://github.com/Thnorty/UnofficialUniwillControlCenter
AppComments=Lighting, performance modes, fan curves and battery care for laptops built by Uniwill (unofficial)
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; The app needs administrator rights for the BIOS and the embedded controller, so it goes in Program Files.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir=..\dist
OutputBaseFilename=UnofficialUniwillControlCenter-{#AppVersion}-Setup
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

[InstallDelete]
; Left from when the app was called Tulpar Control. An update keeps the folder it was installed in.
Type: files; Name: "{app}\TulparControl.exe"
Type: files; Name: "{autoprograms}\Tulpar Control.lnk"
Type: files; Name: "{autodesktop}\Tulpar Control.lnk"

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
; The same startup task as the app's own "Start with Windows" switch, so the switch shows it.
; An update leaves it as the app's switch has it.
Filename: "{app}\{#AppExe}"; Parameters: "--enable-startup"; StatusMsg: "Setting {#AppName} to start with Windows..."; Flags: runhidden waituntilterminated; Tasks: autostart; Check: not IsUpdate
Filename: "{app}\{#AppExe}"; Parameters: "--disable-startup"; Flags: runhidden waituntilterminated; Tasks: not autostart; Check: not IsUpdate
; With Setup's administrator rights: a postinstall entry otherwise runs as the original user, and starting an app that
; requires them that way fails (CreateProcess error 740).
Filename: "{app}\{#AppExe}"; Description: "Open {#AppName}"; Flags: postinstall nowait skipifsilent runascurrentuser

[UninstallRun]
Filename: "{app}\{#AppExe}"; Parameters: "--exit"; RunOnceId: "Exit"; Flags: runhidden waituntilterminated
Filename: "{app}\{#AppExe}"; Parameters: "--disable-startup"; RunOnceId: "DisableStartup"; Flags: runhidden waituntilterminated

[Code]
var
  RemoveData: Boolean;
  // The version already installed, or '' on a first install.
  InstalledVersion: String;

function InitializeSetup: Boolean;
begin
  if not RegQueryStringValue(HKLM, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#AppGuid}_is1', 'DisplayVersion',
    InstalledVersion) then
    InstalledVersion := '';
  Result := True;
end;

function IsUpdate: Boolean;
begin
  Result := InstalledVersion <> '';
end;

// -1 when the installed version is older than this one, 0 the same, 1 newer (or unreadable).
function CompareInstalled: Integer;
var
  Installed, This: Int64;
begin
  if StrToVersion(InstalledVersion, Installed) and StrToVersion('{#AppVersion}', This) then
    Result := ComparePackedVersion(Installed, This)
  else
    Result := 1;
end;

// An update keeps the folder and the choices made when it was installed (the startup switch is the app's own by then),
// so it goes straight to the Ready page, which says what is updated.
function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := IsUpdate and (PageID = wpSelectTasks);
end;

// An update's summary leaves out the tasks, which it doesn't change.
function UpdateReadyMemo(Space, NewLine, MemoUserInfoInfo, MemoDirInfo, MemoTypeInfo, MemoComponentsInfo, MemoGroupInfo,
  MemoTasksInfo: String): String;
begin
  Result := MemoDirInfo;
  if not IsUpdate and (MemoTasksInfo <> '') then
    Result := Result + NewLine + NewLine + MemoTasksInfo;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if not IsUpdate then
    Exit;
  case CurPageID of
    wpReady:
      begin
        if CompareInstalled < 0 then begin
          WizardForm.PageNameLabel.Caption := 'Ready to Update';
          WizardForm.PageDescriptionLabel.Caption := 'Setup is ready to update {#AppName} from ' + InstalledVersion + ' to {#AppVersion}.';
          WizardForm.ReadyLabel.Caption := 'Click Update to go on. Your settings and plugins are kept.';
          WizardForm.NextButton.Caption := '&Update';
        end else if CompareInstalled = 0 then begin
          WizardForm.PageNameLabel.Caption := 'Ready to Reinstall';
          WizardForm.PageDescriptionLabel.Caption := '{#AppName} {#AppVersion} is already installed.';
          WizardForm.ReadyLabel.Caption := 'Click Reinstall to install it again. Your settings and plugins are kept.';
          WizardForm.NextButton.Caption := '&Reinstall';
        end else begin
          WizardForm.PageNameLabel.Caption := 'Ready to Install an Older Version';
          WizardForm.PageDescriptionLabel.Caption := 'The installed ' + InstalledVersion + ' is newer than this {#AppVersion}.';
          WizardForm.ReadyLabel.Caption := 'Click Install to replace it. Your settings and plugins are kept.';
        end;
      end;
    wpInstalling:
      if CompareInstalled < 0 then begin
        WizardForm.PageNameLabel.Caption := 'Updating';
        WizardForm.PageDescriptionLabel.Caption := 'Please wait while Setup updates {#AppName} to {#AppVersion}.';
      end;
    wpFinished:
      if CompareInstalled < 0 then begin
        WizardForm.FinishedHeadingLabel.Caption := '{#AppName} is updated';
        WizardForm.FinishedLabel.Caption := '{#AppName} is updated from ' + InstalledVersion + ' to {#AppVersion}.';
      end;
  end;
end;

// Before files are replaced: quit the running app (whichever copy it is) the way its Exit menu does, so the Fn keys
// and the fans are handed back to the laptop. The new exe is used, since an older one doesn't know --exit.
// That takes a few seconds, so a page says what is going on meanwhile. Setup doesn't wait in Exec, which disables its
// window (a click only plays the error sound): the app and --exit both hold the app's single-instance lock while they
// run, so it waits for the lock to go, the way --exit itself does (up to 30 s).
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code, Waited: Integer;
  Page: TOutputMarqueeProgressWizardPage;
begin
  Result := '';
  Page := CreateOutputMarqueeProgressPage('Closing {#AppName}', 'It hands the Fn keys and the fans back to the laptop first.');
  Page.SetText('Closing {#AppName} if it is running...', '');
  Page.Show;
  try
    Page.Animate;
    ExtractTemporaryFile('{#AppExe}');
    if not Exec(ExpandConstant('{tmp}\{#AppExe}'), '--exit', '', SW_HIDE, ewNoWait, Code) then
      Result := '{#AppName} couldn''t be closed: ' + SysErrorMessage(Code)
    else begin
      Waited := 0;
      while CheckForMutexes('{#AppLock}') and (Waited < 35000) do begin
        Page.Animate;  // lets the window handle input and paint
        Sleep(50);
        Waited := Waited + 50;
      end;
      if CheckForMutexes('{#AppLock}') then
        Result := '{#AppName} is still running. Close it from its tray icon (Exit), then try again.';
    end;
  finally
    Page.Hide;
  end;
end;

function HasParam(Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
      Result := True;
end;

// Once the uninstall is confirmed: whether to remove the settings and the plugins too, ticked to begin with. Without a
// window (/SILENT or /VERYSILENT) they're kept, unless /REMOVEDATA is given.
function AskRemoveData: Boolean;
var
  Form: TSetupForm;
  Text: TNewStaticText;
  Box: TNewCheckBox;
  Button: TNewButton;
begin
  Form := CreateCustomForm(ScaleX(400), ScaleY(128), False, True);
  try
    Form.Caption := '{#AppName} Uninstall';

    Text := TNewStaticText.Create(Form);
    Text.Parent := Form;
    Text.Left := ScaleX(16);
    Text.Top := ScaleY(16);
    Text.Width := Form.ClientWidth - ScaleX(32);
    Text.AutoSize := False;
    Text.WordWrap := True;
    Text.Height := ScaleY(46);
    Text.Caption := 'Your settings and plugins are in ' + ExpandConstant('{userappdata}\UnofficialUniwillControlCenter') + '. ' +
      'Plugins may also have added hooks to Claude Code''s settings.';

    Box := TNewCheckBox.Create(Form);
    Box.Parent := Form;
    Box.Left := ScaleX(16);
    Box.Top := ScaleY(68);
    Box.Width := Form.ClientWidth - ScaleX(32);
    Box.Height := ScaleY(20);
    Box.Caption := 'Remove my settings and plugins too';
    Box.Checked := True;

    Button := TNewButton.Create(Form);
    Button.Parent := Form;
    Button.Caption := 'Continue';
    Button.Width := Form.CalculateButtonWidth([Button.Caption]);
    Button.Height := ScaleY(23);
    Button.Left := Form.ClientWidth - Button.Width - ScaleX(16);
    Button.Top := Form.ClientHeight - Button.Height - ScaleY(12);
    Button.ModalResult := mrOk;
    Button.Default := True;
    Button.Cancel := True;

    Form.ActiveControl := Button;
    Form.ShowModal();
    Result := Box.Checked;
  finally
    Form.Free();
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Code: Integer;
begin
  case CurUninstallStep of
    usAppMutexCheck:
      if UninstallSilent then
        RemoveData := HasParam('/REMOVEDATA')
      else
        RemoveData := AskRemoveData;
    // Before the exe is removed: the app is closed, then it removes the folders and the plugins' Claude Code hooks.
    usUninstall:
      if RemoveData then begin
        Exec(ExpandConstant('{app}\{#AppExe}'), '--exit', '', SW_HIDE, ewWaitUntilTerminated, Code);
        Exec(ExpandConstant('{app}\{#AppExe}'), '--remove-data', '', SW_HIDE, ewWaitUntilTerminated, Code);
      end;
  end;
end;
