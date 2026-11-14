; Per-user installer. Build with: iscc /DAppVersion=0.3.2 installer\nijimi.iss
; Expects a release build in target\release; writes to dist\.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define RepoUrl "https://github.com/sortbyfirstname/nijimi"
; Never change AppGuid: it's how a newer setup finds and upgrades an existing install.
#define AppGuid "4072AE73-3143-4133-A00D-9C396744AFF2"

[Setup]
AppId={{{#AppGuid}}
AppName=nijimi
AppVersion={#AppVersion}
AppVerName=nijimi {#AppVersion}
AppPublisher=Aaron Seabrook
AppPublisherURL={#RepoUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
DefaultDirName={autopf}\nijimi
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
CloseApplications=no
WizardStyle=modern
SetupIconFile=..\assets\tresbient.ico
UninstallDisplayIcon={app}\nijimi.exe
UninstallDisplayName=nijimi
UninstallFilesDir={app}\uninstall
OutputDir=..\dist
OutputBaseFilename=nijimi-v{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\nijimi.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\nijimi"; Filename: "{app}\nijimi.exe"
Name: "{autodesktop}\nijimi"; Filename: "{app}\nijimi.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\nijimi.exe"; Description: "Launch nijimi"; Flags: nowait postinstall skipifsilent
Filename: "{app}\nijimi.exe"; Parameters: "--background"; Flags: nowait; Check: RelaunchAfterSilentUpgrade

[UninstallDelete]
Type: files; Name: "{app}\nijimi.ini"
Type: files; Name: "{app}\nijimi.ini.bad"
Type: files; Name: "{app}\nijimi.tmp"
Type: files; Name: "{app}\.nijimi-write-test-*"

[Code]
const
  TrayWindowClass = 'NijimiTrayWindow';
  InstanceMutexes = 'Local\NijimiAmbientLight,Local\NijimiSettingsWindow';
  // EXIT_COMMAND in src/app.rs: the tray menu's Exit, which also closes Settings.
  ExitCommand = 2;
  WM_COMMAND = $0111;
  SYNCHRONIZE = $00100000;
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  StartupApprovedKey = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run';
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{' + '{#AppGuid}' + '}_is1';

var
  WasRunning: Boolean;

function InitializeSetup(): Boolean;
var
  Installed: String;
  InstalledVersion, NewVersion: Int64;
begin
  Result := True;
  if RegQueryStringValue(HKCU, UninstallKey, 'DisplayVersion', Installed)
    and StrToVersion(Installed, InstalledVersion)
    and StrToVersion('{#AppVersion}', NewVersion)
    and (ComparePackedVersion(InstalledVersion, NewVersion) > 0) then
    Result := SuppressibleMsgBox(
      FmtMessage('nijimi %1 is already installed. Install the older version %2 anyway?', [
        Installed, '{#AppVersion}']),
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES;
end;

function GetWindowThreadProcessId(Wnd: HWND; var ProcessId: DWORD): DWORD;
  external 'GetWindowThreadProcessId@user32.dll stdcall';
function OpenProcess(DesiredAccess: DWORD; InheritHandle: BOOL; ProcessId: DWORD): THandle;
  external 'OpenProcess@kernel32.dll stdcall';
function WaitForSingleObject(Handle: THandle; Milliseconds: DWORD): DWORD;
  external 'WaitForSingleObject@kernel32.dll stdcall';
function CloseHandle(Handle: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

function CloseNijimi(): Boolean;
var
  Tray: HWND;
  ProcessId: DWORD;
  Process: THandle;
  Attempt: Integer;
begin
  Tray := FindWindowByClassName(TrayWindowClass);
  if Tray <> 0 then
  begin
    ProcessId := 0;
    GetWindowThreadProcessId(Tray, ProcessId);
    Process := OpenProcess(SYNCHRONIZE, False, ProcessId);
    PostMessage(Tray, WM_COMMAND, ExitCommand, 0);
    if Process <> 0 then
    begin
      WaitForSingleObject(Process, 10000);
      CloseHandle(Process);
    end;
  end;
  for Attempt := 1 to 50 do
  begin
    if not CheckForMutexes(InstanceMutexes) then
      Break;
    Sleep(100);
  end;
  Result := not CheckForMutexes(InstanceMutexes);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  WasRunning := FindWindowByClassName(TrayWindowClass) <> 0;
  if not CloseNijimi() then
    Result := 'Setup couldn''t close nijimi. Exit it from its tray icon, then click Back and try again.';
end;

function RelaunchAfterSilentUpgrade(): Boolean;
begin
  Result := WizardSilent() and WasRunning;
end;

function InitializeUninstall(): Boolean;
begin
  Result := CloseNijimi();
  if not Result then
    SuppressibleMsgBox('Couldn''t close nijimi. Exit it from its tray icon, then uninstall again.',
      mbError, MB_OK, IDOK);
end;

// Only removes the startup entry if it points at this install, not at a portable copy elsewhere.
procedure RemoveStartupEntry();
var
  Command: String;
begin
  if RegQueryStringValue(HKCU, RunKey, 'nijimi', Command)
    and (Pos(Lowercase(ExpandConstant('{app}\nijimi.exe')), Lowercase(Command)) > 0) then
  begin
    RegDeleteValue(HKCU, RunKey, 'nijimi');
    RegDeleteValue(HKCU, StartupApprovedKey, 'nijimi');
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveStartupEntry();
end;
