; Inno Setup script for the Windows installer.
;   iscc /DAppVersion=0.2.0 /DExeSource=..\..\target\release\superfastimageview.exe packaging\windows\installer.iss
; Installs per user by default (no admin prompt); the first page offers an
; all-users install instead. Adds SuperFastImageView to "Open with" for every
; supported image type without taking over the current default app.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef ExeSource
  #define ExeSource "..\..\target\release\superfastimageview.exe"
#endif
#define AppName "SuperFastImageView"
#define ExeName "SuperFastImageView.exe"
#define ProgId "SuperFastImageView.Image"

[Setup]
AppId={{6F4B2C1E-8A3D-4E7B-9C5F-2D1A0B3E4F56}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=SuperFastImageView
AppPublisherURL=https://github.com/darshan5/superfastimageview
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\{#ExeName}
SetupIconFile=..\..\assets\icon.ico
LicenseFile=..\..\LICENSE
OutputDir=..\..\dist
OutputBaseFilename=SuperFastImageView-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#ExeSource}"; DestDir: "{app}"; DestName: "{#ExeName}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"
Source: "..\..\assets\icons-LICENSE.txt"; DestDir: "{app}"

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#ExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#ExeName}"; Tasks: desktopicon

[Registry]
; The file type that "Open with" entries point at.
Root: HKA; Subkey: "Software\Classes\{#ProgId}"; ValueType: string; ValueName: ""; ValueData: "Image"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\{#ProgId}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\{#ExeName}"",0"
Root: HKA; Subkey: "Software\Classes\{#ProgId}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#ExeName}"" ""%1"""
; Every supported extension (loader.rs, raw.rs, platform.rs).
Root: HKA; Subkey: "Software\Classes\.jpg\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.jpeg\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.jpe\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.jfif\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.png\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.apng\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.gif\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.bmp\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.dib\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ico\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.tif\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.tiff\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.webp\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.tga\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.qoi\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pbm\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pgm\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ppm\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pnm\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pam\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.hdr\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.exr\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.dds\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ff\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.3fr\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ari\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.arw\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.cr2\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.cr3\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.crw\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.dcr\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.dng\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.erf\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.fff\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.iiq\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.kdc\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.mef\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.mos\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.mrw\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.nef\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.nrw\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.orf\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ori\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pef\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.raf\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.raw\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.rw2\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.rwl\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.sr2\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.srf\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.srw\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.x3f\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.heic\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.heif\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.hif\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.avif\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".jpg"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".jpeg"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".jpe"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".jfif"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".png"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".apng"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".gif"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".bmp"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".dib"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".ico"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".tif"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".tiff"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".webp"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".tga"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".qoi"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".pbm"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".pgm"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".ppm"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".pnm"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".pam"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".hdr"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".exr"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".dds"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".ff"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".3fr"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".ari"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".arw"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".cr2"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".cr3"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".crw"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".dcr"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".dng"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".erf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".fff"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".iiq"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".kdc"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".mef"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".mos"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".mrw"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".nef"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".nrw"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".orf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".ori"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".pef"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".raf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".raw"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".rw2"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".rwl"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".sr2"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".srf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".srw"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".x3f"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".heic"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".heif"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".hif"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".avif"; ValueData: ""

[Run]
Filename: "{app}\{#ExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
