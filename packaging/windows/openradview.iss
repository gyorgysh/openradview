; SPDX-License-Identifier: GPL-3.0-or-later
; Keep uninstall metadata outside the payload replaced by in-app updates.
[Setup]
AppId={{46954E76-B024-4C31-988A-7C54D61CB1F5}
AppName=OpenRadView
AppVersion={#AppVersion}
AppPublisher=gyorgy.sh
AppPublisherURL=https://openradview.com
AppSupportURL=https://github.com/gyorgysh/openradview/issues
AppUpdatesURL=https://github.com/gyorgysh/openradview/releases/latest
DefaultDirName={localappdata}\Programs\OpenRadView
DefaultGroupName=OpenRadView
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
WizardStyle=modern dark
WizardImageFile=
WizardSmallImageFile={#BrandImage}
WizardSmallImageBackColor=$0A0A0A
SetupIconFile={#BrandIcon}
UninstallDisplayIcon={app}\OpenRadView.ico
LicenseFile={#LicenseFile}
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBase}
Compression=lzma2/normal
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
DisableProgramGroupPage=yes
UsePreviousAppDir=yes
SetupLogging=no

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#PayloadDir}\*"; DestDir: "{app}\app"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#BrandIcon}"; DestDir: "{app}"; DestName: "OpenRadView.ico"; Flags: ignoreversion

[Icons]
Name: "{group}\OpenRadView"; Filename: "{app}\app\openradview.exe"; WorkingDir: "{app}\app"; IconFilename: "{app}\OpenRadView.ico"
Name: "{autodesktop}\OpenRadView"; Filename: "{app}\app\openradview.exe"; WorkingDir: "{app}\app"; IconFilename: "{app}\OpenRadView.ico"; Tasks: desktopicon

[UninstallDelete]
Type: dirifempty; Name: "{app}\app"
