; Inno Setup 6 script for kool-cpu (bundles kool-cpu-gui, kool-cpu CLI, and PawnIO driver)
#ifndef AppVersion
  #define AppVersion "0.2.0"
#endif
#ifndef BinDir
  #define BinDir "..\..\target\release"
#endif
#ifndef PawnIoSetup
  #define PawnIoSetup "..\..\target\installer\PawnIO_setup.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{8F6D5A91-3C41-4A72-9D1C-5E7B8B92E104}
AppName=kool-cpu
AppVersion={#AppVersion}
AppPublisher=Heshan Jayakody
AppPublisherURL=https://github.com/heshan-jj/kool-cpu
AppSupportURL=https://github.com/heshan-jj/kool-cpu/issues
DefaultDirName={autopf}\kool-cpu
DefaultGroupName=kool-cpu
DisableProgramGroupPage=yes
LicenseFile=..\..\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename=kool-cpu-setup
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
WizardStyle=modern
CloseApplications=force
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BinDir}\kool-cpu-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\kool-cpu.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
; Official Microsoft-signed PawnIO installer (only executed if PawnIO is not already installed)
Source: "{#PawnIoSetup}"; DestDir: "{tmp}"; Flags: deleteafterinstall

[Icons]
Name: "{group}\kool-cpu"; Filename: "{app}\kool-cpu-gui.exe"
Name: "{group}\Uninstall kool-cpu"; Filename: "{uninstallexe}"
Name: "{autodesktop}\kool-cpu"; Filename: "{app}\kool-cpu-gui.exe"; Tasks: desktopicon

[Run]
Filename: "{tmp}\PawnIO_setup.exe"; Parameters: "-install -silent"; StatusMsg: "Installing PawnIO sensor driver..."; Flags: runhidden waituntilterminated; Check: NeedsPawnIO
Filename: "{app}\kool-cpu-gui.exe"; Description: "Launch kool-cpu"; Flags: nowait postinstall skipifsilent shellexec

[UninstallRun]
; Remove Scheduled Task created by "Run on startup" if present
Filename: "schtasks.exe"; Parameters: "/Delete /TN ""kool-cpu"" /F"; Flags: runhidden; RunOnceId: "RemoveKoolCpuTask"

[Code]
function NeedsPawnIO: Boolean;
begin
  Result := not FileExists(ExpandConstant('{commonpf64}\PawnIO\PawnIOLib.dll'));
end;
