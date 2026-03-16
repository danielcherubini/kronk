[Setup]
AppName=Kronk
AppVersion={#GetEnv("KRONK_VERSION")}
DefaultDirName={autopf}\Kronk
DefaultGroupName=Kronk
OutputDir=..\dist
OutputBaseFilename=kronk-{#GetEnv("KRONK_VERSION")}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64
PrivilegesRequired=admin
LicenseFile=..\LICENSE
ChangesEnvironment=yes
UninstallDisplayName=Kronk

[Files]
Source: "..\target\release\kronk.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Kronk Config"; Filename: "notepad.exe"; Parameters: """{userappdata}\kronk\config\config.toml"""
Name: "{group}\Uninstall Kronk"; Filename: "{uninstallexe}"

[Registry]
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Check: NeedsAddPath('{app}')

[Run]
Filename: "notepad.exe"; Parameters: """{userappdata}\kronk\config\config.toml"""; \
    Description: "Edit configuration"; Flags: postinstall nowait skipifsilent unchecked

[UninstallRun]
Filename: "{app}\kronk.exe"; Parameters: "service remove"; Flags: runhidden

[Code]
function NeedsAddPath(Param: string): boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_LOCAL_MACHINE,
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Param + ';', ';' + OrigPath + ';') = 0;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  OrigPath: string;
  NewPath: string;
  AppDir: string;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    AppDir := ExpandConstant('{app}');
    if RegQueryStringValue(HKEY_LOCAL_MACHINE,
      'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
      'Path', OrigPath) then
    begin
      NewPath := OrigPath;
      StringChangeEx(NewPath, ';' + AppDir, '', True);
      StringChangeEx(NewPath, AppDir + ';', '', True);
      StringChangeEx(NewPath, AppDir, '', True);
      if NewPath <> OrigPath then
        RegWriteStringValue(HKEY_LOCAL_MACHINE,
          'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
          'Path', NewPath);
    end;
  end;
end;
