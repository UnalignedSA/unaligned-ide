; The Windows installer of the product (Inno Setup 6). Written for this fork.
;
; It is compiled by script/unaligned/bundle-windows.ps1, which passes every
; name on the command line (/DName=value) from crates/branding/brand.env:
;
;   AppGuid             WINDOWS_INSTALLER_GUID: what Windows knows the installation by.
;                       It must never change once a release is out, or an update
;                       would install a second copy beside the first.
;   AppName             PRODUCT_NAME
;   AppExeName          <APP_ID>.exe
;   AppVersion          the version of the `ide` package
;   AppPublisher        PUBLISHER
;   AppUrl              WEBSITE_URL
;   AppMutex            <APP_ID>-Stable-Instance-Mutex, the mutex the running editor
;                       holds (crates/ide/src/app/windows_only_instance.rs): Setup
;                       asks to close the editor before it replaces its files.
;   UrlScheme           URL_SCHEME: links of that scheme start the editor.
;   SourceDir           the staged application folder
;   OutputDir           where the installer is written
;   OutputBaseFilename  its file name, without .exe
;   IconFile            the .ico of the installer
;
; The installation is per user: no administrator rights are asked, and the
; files go to %LOCALAPPDATA%\Programs\<AppName>. Nothing is signed.

#ifndef AppGuid
  #error "This file is compiled by script/unaligned/bundle-windows.ps1, which passes the names."
#endif

[Setup]
AppId={{{#AppGuid}}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseFilename}
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\{#AppExeName}
UninstallDisplayName={#AppName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
AppMutex={#AppMutex}
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Registry]
; Links of the product's own scheme start the editor.
Root: HKA; Subkey: "Software\Classes\{#UrlScheme}"; ValueType: string; ValueName: ""; ValueData: "URL:{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\{#UrlScheme}"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\{#UrlScheme}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
; An update the editor started itself (crates/auto_update/src/install.rs runs
; Setup with /SILENT /update=true once the editor has quit): the new version is
; started again without a question.
Filename: "{app}\{#AppExeName}"; Flags: nowait; Check: IsUpdate

[Code]
function IsUpdate(): Boolean;
begin
  Result := ExpandConstant('{param:update|false}') = 'true';
end;
