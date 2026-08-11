; Ward's installer.
;
; Per-user, and that is not a convenience. Everything Ward writes lives in one
; folder beside the executable, and a machine-wide location under Program Files
; is not writable by a standard user — which would break the data folder rather
; than merely relocate it. Installing per-user means no elevation prompt, no
; administrator, and a data folder the running program can actually write to.
;
; The AppId is fixed forever. It is how Windows knows a new version is the same
; application as the old one, so an upgrade replaces an install rather than
; sitting beside it. Changing it would leave every existing Commander with two
; Wards and no way to tell which one their shortcut points at.
;
; Unsigned, deliberately. SmartScreen shows one dialog on first run; a
; certificate is not bought for 0.1.0. The answer to "should I trust this" is a
; published checksum, a public repository, and a build anybody can reproduce.

#define AppName "Ward"
#define AppPublisher "Doug Seelinger"
#define AppUrl "https://github.com/dseelinger/ward"
#define AppExe "ward.exe"

; Passed in by the release build. The fallback is only so the script can be
; opened and read without one.
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{8C5F2A31-7B4D-4E96-9A2C-1D3F5E7B9C04}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases

; Lowest, so no elevation prompt appears and the install lands somewhere the
; Commander owns.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=no

OutputDir=..\dist
OutputBaseFilename=ward-setup-{#AppVersion}
SetupIconFile=
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

; Ward is Windows-only and 64-bit only.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

UninstallDisplayName={#AppName} {#AppVersion}
UninstallDisplayIcon={app}\{#AppExe}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a shortcut on the desktop"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

; Valve's, and the one file Ward cannot link into itself. Windows resolves a
; DLL from the directory of the running executable, so this has to sit beside
; ward.exe rather than anywhere tidier. Without it the program does not start,
; and the error names nothing that suggests why.
Source: "..\target\release\openvr_api.dll"; DestDir: "{app}"; Flags: ignoreversion

; What Ward's controller bindings are, which SteamVR reads rather than Ward. The
; interface that needed no manifest is no longer answered, so without these the
; panel cannot be grabbed at all - and SteamVR's own binding interface has no
; entry for Ward, which is where a Commander would go to change the grip to
; something else.
;
; Beside the executable because that is where Ward looks, and Ward looks there
; because SteamVR resolves the path it is handed against its own working
; directory rather than ours.
Source: "..\target\release\input\*"; DestDir: "{app}\input"; Flags: ignoreversion

Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\NOTICE.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\crates\openvr-sys\vendor\LICENSE"; DestDir: "{app}"; DestName: "LICENSE-OpenVR.txt"; Flags: ignoreversion

[Dirs]
; The one writable folder, and the one thing an upgrade must not touch. Settings,
; the checklist, the stored key and the logs are all in here. Listed with
; uninsneveruninstall so that removing Ward leaves the Commander's own data
; behind rather than taking it with them.
Name: "{app}\data"; Flags: uninsneveruninstall

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Everything the installer put down goes. The data folder above does not, and
; neither does anything Ward wrote into it while running.
Type: files; Name: "{app}\{#AppExe}"
Type: files; Name: "{app}\openvr_api.dll"
