; Cutline – Windows-Installer (Inno Setup 6)
; Installiert pro Benutzer ohne Administratorrechte, damit der eingebaute Updater
; die Programmdateien ersetzen kann.
;
; Bauen:  iscc /DAppVersion=0.7.1 /DSrcDir=..\pkg installer\cutline.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SrcDir
  #define SrcDir "..\pkg"
#endif

[Setup]
AppId={{55642BD8-4DC5-482E-8BD3-4B4885E46728}
AppName=Cutline
AppVersion={#AppVersion}
AppVerName=Cutline {#AppVersion}
AppPublisher=Cutline contributors
AppPublisherURL=https://github.com/zqqqqx/cutline
AppSupportURL=https://github.com/zqqqqx/cutline/issues
AppUpdatesURL=https://github.com/zqqqqx/cutline/releases
DefaultDirName={localappdata}\Programs\Cutline
DefaultGroupName=Cutline
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..
OutputBaseFilename=cutline-windows-x64-setup
SetupIconFile=..\assets\cutline.ico
UninstallDisplayIcon={app}\cutline.exe
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
CloseApplications=yes

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[CustomMessages]
en.AssocTask=Open .cutline project files with Cutline
de.AssocTask=.cutline-Projektdateien mit Cutline öffnen
en.ProjectType=Cutline project
de.ProjectType=Cutline-Projekt

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "assoc"; Description: "{cm:AssocTask}"

[Files]
Source: "{#SrcDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Cutline"; Filename: "{app}\cutline.exe"
Name: "{autodesktop}\Cutline"; Filename: "{app}\cutline.exe"; Tasks: desktopicon

[Registry]
; Dateizuordnung pro Benutzer (HKCU über HKA bei Installation ohne Adminrechte)
Root: HKA; Subkey: "Software\Classes\.cutline"; ValueType: string; ValueName: ""; ValueData: "Cutline.Project"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.cutline\OpenWithProgids"; ValueType: string; ValueName: "Cutline.Project"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Cutline.Project"; ValueType: string; ValueName: ""; ValueData: "{cm:ProjectType}"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Cutline.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\cutline.exe,0"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Cutline.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\cutline.exe"" ""%1"""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\cutline.exe\SupportedTypes"; ValueType: string; ValueName: ".cutline"; ValueData: ""; Flags: uninsdeletekey

[Run]
Filename: "{app}\cutline.exe"; Description: "{cm:LaunchProgram,Cutline}"; Flags: nowait postinstall skipifsilent
