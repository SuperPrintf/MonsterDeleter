#define AppName "Monster Deleter"
#define AppVersion "1.0.17"
#define AppExeName "monster-deleter.exe"

[Setup]
AppId={{1F5473BE-201D-4F5E-ABB3-3FA0D679C083}
AppName={#AppName}
AppVersion={#AppVersion}
DefaultDirName={autopf}\MonsterDeleter
DefaultGroupName={#AppName}
UninstallDisplayName={#AppName}
OutputDir=..\dist
OutputBaseFilename=MonsterDeleter-Setup-{#AppVersion}
SetupIconFile=..\assets\branding\monster-head-v2.ico
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64os

[Tasks]
Name: "open_installation_guide"; Description: "打开安装与卸载说明（推荐）"
Name: "enable_uninstall_feature"; Description: "启用卸载功能（识别软件快捷方式后提供卸载询问）"
Name: "build_uninstall_index"; Description: "首次建立软件卸载索引（可能延长安装时间）"; Flags: unchecked
Name: "uninstall_silent_execution"; Description: "卸载功能静默执行（可能跳过软件自身的卸载界面）"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\docs\安装与卸载说明.txt"; DestDir: "{app}"; Flags: ignoreversion

[Run]
Filename: "{app}\assets\tools\bcu-bridge\bcu-bridge.exe"; Parameters: "index ""{userappdata}\MonsterDeleter\uninstall-index.tsv"""; Flags: runhidden waituntilterminated skipifsilent; Check: ShouldBuildUninstallIndex
Filename: "notepad.exe"; Parameters: """{app}\安装与卸载说明.txt"""; Flags: nowait skipifsilent; Tasks: open_installation_guide

[Icons]
Name: "{group}\Monster Deleter 设置"; Filename: "{app}\{#AppExeName}"; Parameters: "--settings"; WorkingDir: "{app}"

[Registry]
Root: HKLM; Subkey: "Software\Classes\*\shell\MonsterDeleter"; ValueType: string; ValueName: ""; ValueData: "召唤小怪兽删除"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\*\shell\MonsterDeleter"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\assets\branding\monster-head-v2.ico"
Root: HKLM; Subkey: "Software\Classes\*\shell\MonsterDeleter\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""
; Explorer resolves %1 to a shortcut's target for wildcard verbs. Register the
; same verb on lnkfile so shortcut invocations receive the .lnk path itself.
Root: HKLM; Subkey: "Software\Classes\lnkfile\shell\MonsterDeleter"; ValueType: string; ValueName: ""; ValueData: "召唤小怪兽删除"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\lnkfile\shell\MonsterDeleter"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\assets\branding\monster-head-v2.ico"
Root: HKLM; Subkey: "Software\Classes\lnkfile\shell\MonsterDeleter\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\Directory\shell\MonsterDeleter"; ValueType: string; ValueName: ""; ValueData: "召唤小怪兽删除"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\Directory\shell\MonsterDeleter"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\assets\branding\monster-head-v2.ico"
Root: HKLM; Subkey: "Software\Classes\Directory\shell\MonsterDeleter\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""

[Code]
procedure SHChangeNotify(wEventId: Integer; uFlags: Cardinal; dwItem1, dwItem2: Integer);
  external 'SHChangeNotify@shell32.dll stdcall';

procedure WriteInitialUninstallConfig;
var
  ConfigPath: String;
  UninstallEnabled: Boolean;
  SilentExecution: Boolean;
  EnabledText: String;
  Mode: String;
begin
  UninstallEnabled := WizardIsTaskSelected('enable_uninstall_feature');
  SilentExecution := UninstallEnabled and WizardIsTaskSelected('uninstall_silent_execution');
  if UninstallEnabled then
    EnabledText := 'true'
  else
    EnabledText := 'false';
  if SilentExecution then
    Mode := 'silent'
  else
    Mode := 'official';
  ConfigPath := ExpandConstant('{userappdata}\MonsterDeleter\config.json');
  ForceDirectories(ExtractFileDir(ConfigPath));
  SaveStringToFile(ConfigPath,
    '{' + #13#10 +
    '  "uninstall": {' + #13#10 +
    '    "enabled": ' + EnabledText + ',' + #13#10 +
    '    "mode": "' + Mode + '",' + #13#10 +
    '    "target_patterns": [' + #13#10 +
    '      "(?i)^.*\\.lnk$",' + #13#10 +
    '      "(?i)^.*\\.exe$"' + #13#10 +
    '    ],' + #13#10 +
    '    "cleanup_after_uninstall": false' + #13#10 +
    '  }' + #13#10 +
    '}' + #13#10, False);
end;

function ShouldBuildUninstallIndex(): Boolean;
begin
  Result := WizardIsTaskSelected('enable_uninstall_feature') and
    WizardIsTaskSelected('build_uninstall_index');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    WriteInitialUninstallConfig;
    SHChangeNotify($08000000, 0, 0, 0);
    if not WizardSilent then
      MsgBox('安装完成！' + #13#10#13#10 +
        '“小怪兽删除器”已添加到资源管理器中文件和文件夹的右键菜单。' + #13#10 +
        '请在目标文件或文件夹上右键，然后选择“召唤小怪兽删除”。' + #13#10#13#10 +
        '可从开始菜单打开“Monster Deleter 设置”，启用或关闭卸载功能，并选择官方卸载或“卸载功能静默执行”；默认启用卸载功能、使用官方卸载，且不会清理残留项。' + #13#10#13#10 +
        '如需卸载本程序，请打开“控制面板 → 程序和功能”，选择 Monster Deleter 后单击“卸载”。' + #13#10 +
        '卸载会同时移除右键菜单入口。', mbInformation, MB_OK);
  end;
end;
