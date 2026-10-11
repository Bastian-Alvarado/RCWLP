; RCWLP host installer (Inno Setup 6). Build it with installer\build.ps1, which compiles the
; host and passes the version from host\Cargo.toml.
;
; Installs for the current user only, without admin rights: the tray app (and the console host
; for troubleshooting) under %LOCALAPPDATA%\Programs\RCWLP, a Start Menu entry, and "start when
; I sign in". The ViGEmBus driver is checked for, not installed; it needs admin rights.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppName "RCWLP host"
#define TrayExe "rcwlp-tray.exe"
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"
#define DriverUrl "https://github.com/nefarius/ViGEmBus/releases"

[Setup]
AppId={{9A04CFE5-601C-4737-9663-2AC4CE10E1C6}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=RCWLP
DefaultDirName={autopf}\RCWLP
DefaultGroupName=RCWLP
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=output
OutputBaseFilename=RCWLP-host-setup-{#AppVersion}
SetupIconFile=..\host\assets\rcwlp.ico
UninstallDisplayIcon={app}\{#TrayExe}
UninstallDisplayName={#AppName}
VersionInfoVersion={#AppVersion}
; SimonSays's look: dark, with the RCWLP tile (images drawn by host\assets\make-icons.ps1).
WizardStyle=modern dark
WizardBackColor=#292929
WizardImageFile=wizard-164.png,wizard-328.png
WizardSmallImageFile=wizard-small-58.png,wizard-small-116.png
Compression=lzma2
SolidCompression=yes
; The tray app is stopped by the [Code] below; nothing else of ours should be running.
CloseApplications=no

[Tasks]
Name: autostart; Description: "Start {#AppName} when I sign in (recommended)"

[Files]
Source: "..\host\target\release\{#TrayExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\host\target\release\rcwlp-host.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#TrayExe}"; Comment: "Use Android phones with controllers as game controllers on this PC"

[Registry]
; The same value the tray's "Start with Windows" menu item sets, so the two agree.
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "RCWLP"; ValueData: """{app}\{#TrayExe}"""; Tasks: autostart

[Run]
Filename: "{app}\{#TrayExe}"; Description: "Start {#AppName} now"; Flags: nowait postinstall; Check: DriverInstalled

[Messages]
FinishedLabel=Setup has installed [name]. It runs in the notification area, next to the clock.%n%nThe first time it runs, Windows may ask whether it may use the network. Allow it, at least on private networks, or phones won't find this PC.

[Code]
function DriverInstalled: Boolean;
begin
  Result := RegKeyExists(HKLM, 'SYSTEM\CurrentControlSet\Services\ViGEmBus');
end;

{ Ends a running tray app, so its file can be replaced or removed. Its virtual controllers
  are unplugged when the process ends. }
procedure StopTray;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/f /im {#TrayExe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopTray;
  Result := '';
end;

procedure OpenDriverPage;
var
  Code: Integer;
begin
  ShellExec('open', '{#DriverUrl}', '', '', SW_SHOWNORMAL, ewNoWait, Code);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep <> ssPostInstall then
    Exit;
  { Unticking the task on a reinstall turns starting at sign-in off again. }
  if not WizardIsTaskSelected('autostart') then
    RegDeleteValue(HKCU, '{#RunKey}', 'RCWLP');
  if not DriverInstalled and not WizardSilent then
    if MsgBox('{#AppName} needs the ViGEmBus driver to create virtual controllers, and it isn''t installed yet.' + #13#10#13#10 +
              'Install it from its download page (it needs admin rights), then start {#AppName} from the Start Menu.' + #13#10#13#10 +
              'Open the download page now?', mbConfirmation, MB_YESNO) = IDYES then
      OpenDriverPage;
end;

function InitializeUninstall: Boolean;
begin
  StopTray;
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  { The tray's own menu may have set this too. }
  RegDeleteValue(HKCU, '{#RunKey}', 'RCWLP');
  if DirExists(ExpandConstant('{userappdata}\RCWLP')) and not UninstallSilent then
    if MsgBox('Also forget the phones paired with this PC?' + #13#10#13#10 +
              'Keep them if you will install {#AppName} again.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(ExpandConstant('{userappdata}\RCWLP'), True, True, True);
end;
