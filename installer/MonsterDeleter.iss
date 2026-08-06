#define AppName "Monster Deleter"
#define AppVersion "1.0.4"
#define AppExeName "monster-deleter.exe"

[Setup]
AppId={{1F5473BE-201D-4F5E-ABB3-3FA0D679C083}
AppName={#AppName}
AppVersion={#AppVersion}
DefaultDirName={autopf}\MonsterDeleter
DefaultGroupName={#AppName}
UninstallDisplayName={#AppName}
OutputDir=..\dist
OutputBaseFilename=MonsterDeleter-Setup
SetupIconFile=..\assets\branding\monster-head-v2.ico
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64os

[Files]
Source: "..\target\release\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs

[Registry]
Root: HKLM; Subkey: "Software\Classes\*\shell\MonsterDeleter"; ValueType: string; ValueName: ""; ValueData: "召唤小怪兽删除"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\*\shell\MonsterDeleter"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\assets\branding\monster-head-v2.ico"
Root: HKLM; Subkey: "Software\Classes\*\shell\MonsterDeleter\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\Directory\shell\MonsterDeleter"; ValueType: string; ValueName: ""; ValueData: "召唤小怪兽删除"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\Directory\shell\MonsterDeleter"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\assets\branding\monster-head-v2.ico"
Root: HKLM; Subkey: "Software\Classes\Directory\shell\MonsterDeleter\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""

[Code]
procedure SHChangeNotify(wEventId: Integer; uFlags: Cardinal; dwItem1, dwItem2: Integer);
  external 'SHChangeNotify@shell32.dll stdcall';

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    SHChangeNotify($08000000, 0, 0, 0);
end;
