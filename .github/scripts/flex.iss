; The Windows installer, made by package.sh from the release folder:
;
;   ISCC /DAppVersion=0.2.0 /DSourceDir=<release folder> /DOutputDir=<dir> /DOutputName=<name> flex.iss
;
; It installs for the current user (no admin prompt) and is also the way to update: if
; Flex is running (maybe hidden in the tray), it's asked to quit first and started again
; afterwards. Libraries and history live in %APPDATA%\Flex and are never touched.

#ifndef AppVersion
  #error AppVersion isn't defined
#endif

[Setup]
; Never change AppId: it's how a new version finds and replaces the old one.
AppId={{E1F09833-3DD3-4B0D-9940-2ECAEF3474BC}
AppName=Flex
AppVersion={#AppVersion}
AppVerName=Flex {#AppVersion}
AppPublisher=Caitlin Wilks
AppPublisherURL=https://github.com/catchouli/flex
DefaultDirName={autopf}\Flex
DefaultGroupName=Flex
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
; Flex is stopped by PrepareToInstall below, not by the Restart Manager's prompt.
CloseApplications=no
SetupIconFile={#SourcePath}\..\..\assets\flex.ico
UninstallDisplayIcon={app}\flex.exe
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[InstallDelete]
; DLLs of an older ffmpeg or libmpv that this version doesn't have any more.
Type: files; Name: "{app}\*.dll"

[Files]
Source: "{#SourceDir}\flex.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\*"; Excludes: "flex.exe"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Flex"; Filename: "{app}\flex.exe"
Name: "{autodesktop}\Flex"; Filename: "{app}\flex.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\flex.exe"; Description: "Start Flex"; Flags: nowait postinstall skipifsilent
; A silent update (/SILENT or /VERYSILENT) puts back a Flex that was running.
Filename: "{app}\flex.exe"; Flags: nowait; Check: RestartAfterSilentInstall

[Code]
var
  WasRunning: Boolean;

function FlexRunning: Boolean;
var
  Locator, Service, Found: Variant;
begin
  Result := False;
  try
    Locator := CreateOleObject('WbemScripting.SWbemLocator');
    Service := Locator.ConnectServer('.', 'root\CIMV2');
    Found := Service.ExecQuery('SELECT ProcessId FROM Win32_Process WHERE Name = ''flex.exe''');
    Result := Found.Count > 0;
  except
    Log('Couldn''t check for a running Flex: ' + GetExceptionMessage);
  end;
end;

{ Asks Flex to quit (it saves what's playing first), and closes it if it doesn't. }
procedure StopFlex(Exe: String);
var
  Code, Waited: Integer;
begin
  if not FlexRunning then
    Exit;
  WasRunning := True;
  Log('Flex is running; asking it to quit');
  Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Waited := 0;
  while FlexRunning and (Waited < 15000) do begin
    Sleep(250);
    Waited := Waited + 250;
  end;
  if FlexRunning then begin
    Log('Flex didn''t quit; closing it');
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM flex.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Sleep(500);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  { The new flex.exe knows how to ask any version to quit, wherever it's installed. }
  ExtractTemporaryFile('flex.exe');
  StopFlex(ExpandConstant('{tmp}\flex.exe'));
  if FlexRunning then
    Result := 'Flex is still running. Quit it (right-click its tray icon, Quit) and try again.';
end;

function RestartAfterSilentInstall: Boolean;
begin
  Result := WizardSilent and WasRunning;
end;

function InitializeUninstall: Boolean;
begin
  StopFlex(ExpandConstant('{app}\flex.exe'));
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  { "Start with Windows" would otherwise point at the removed flex.exe. }
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Flex');
end;
