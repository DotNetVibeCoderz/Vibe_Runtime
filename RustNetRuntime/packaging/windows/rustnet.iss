; RustNetRuntime installer for Windows (Inno Setup 6).
; Built by Gravicode Studios, led by Kang Fadhil.
;
;   iscc /DAppVersion=0.1.1 /DStageDir=..\..\dist\rustnet-0.1.1-win-x64 /DArch=x64 /DOutputDir=..\..\dist rustnet.iss
;
; StageDir is a tree made by packaging/build.sh: bin\rustnet.exe, codegen\ (optional),
; docs\, samples\, README*, LICENSE, VERSION. Installs per user by default (no elevation);
; the user can choose an all-users install, which asks for elevation.

#ifndef AppVersion
  #define AppVersion "0.1.1"
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef StageDir
  #define StageDir "..\..\dist\rustnet-" + AppVersion + "-win-" + Arch
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif
#define HasCodeGen FileExists(StageDir + "\codegen\CodeGen.exe")
#if Arch == "x64"
  #define ArchAllowed "x64compatible"
#else
  #define ArchAllowed Arch
#endif

[Setup]
AppId={{6E0B2B6A-6C3B-4F2B-9D7E-52A4C1E0F3B1}
AppName=RustNetRuntime
AppVersion={#AppVersion}
AppVerName=RustNetRuntime {#AppVersion}
AppPublisher=Gravicode Studios
AppPublisherURL=https://github.com/DotNetVibeCoderz/Vibe_Runtime
AppSupportURL=https://github.com/DotNetVibeCoderz/Vibe_Runtime/issues
AppUpdatesURL=https://github.com/DotNetVibeCoderz/Vibe_Runtime/releases
AppCopyright=Gravicode Studios, led by Kang Fadhil
VersionInfoVersion={#AppVersion}
VersionInfoDescription=RustNetRuntime setup
DefaultDirName={autopf}\RustNetRuntime
DefaultGroupName=RustNetRuntime
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed={#ArchAllowed}
ArchitecturesInstallIn64BitMode={#ArchAllowed}
ChangesEnvironment=yes
LicenseFile={#StageDir}\LICENSE
SetupIconFile=..\icon\rustnet.ico
UninstallDisplayIcon={app}\rustnet.ico
UninstallDisplayName=RustNetRuntime {#AppVersion}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename=RustNetRuntime-{#AppVersion}-win-{#Arch}-setup

[Messages]
WelcomeLabel2=This installs RustNetRuntime [name/ver]: the rustnet toolchain that runs .NET assemblies on RustCLR, a .NET runtime written in Rust{#HasCodeGen ? ", and the CodeGen IDE" : ""}.%n%nYou also need the .NET SDK to compile C#.%n%nBuilt by Gravicode Studios, led by Kang Fadhil.%nDibuat oleh Gravicode Studios dipimpin oleh Kang Fadhil.

[Tasks]
Name: "addtopath"; Description: "Add rustnet to PATH"; GroupDescription: "Command line:"
#if HasCodeGen
Name: "desktopicon"; Description: "Create a desktop shortcut for CodeGen"; GroupDescription: "Shortcuts:"; Flags: unchecked
#endif

[Files]
Source: "{#StageDir}\bin\*"; DestDir: "{app}\bin"; Flags: ignoreversion recursesubdirs
#if HasCodeGen
Source: "{#StageDir}\codegen\*"; DestDir: "{app}\codegen"; Flags: ignoreversion recursesubdirs
#endif
Source: "{#StageDir}\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs
Source: "{#StageDir}\samples\*"; DestDir: "{app}\samples"; Flags: ignoreversion recursesubdirs
Source: "{#StageDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\README.id.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\VERSION"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\icon\rustnet.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
#if HasCodeGen
Name: "{group}\CodeGen"; Filename: "{app}\codegen\CodeGen.exe"; IconFilename: "{app}\rustnet.ico"; Comment: "The RustNetRuntime IDE"
Name: "{autodesktop}\CodeGen"; Filename: "{app}\codegen\CodeGen.exe"; IconFilename: "{app}\rustnet.ico"; Tasks: desktopicon
#endif
Name: "{group}\RustNetRuntime command prompt"; Filename: "{cmd}"; Parameters: "/k ""set PATH={app}\bin;%PATH%&& rustnet --version"""; WorkingDir: "{userdocs}"; IconFilename: "{app}\rustnet.ico"
Name: "{group}\Documentation"; Filename: "{app}\docs"
Name: "{group}\Uninstall RustNetRuntime"; Filename: "{uninstallexe}"

[Run]
#if HasCodeGen
Filename: "{app}\codegen\CodeGen.exe"; Description: "Open CodeGen"; Flags: nowait postinstall skipifsilent unchecked
#endif

[Code]
const
  UserEnvKey = 'Environment';
  SystemEnvKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';

function EnvRoot: Integer;
begin
  if IsAdminInstallMode then Result := HKEY_LOCAL_MACHINE else Result := HKEY_CURRENT_USER;
end;

function EnvKey: String;
begin
  if IsAdminInstallMode then Result := SystemEnvKey else Result := UserEnvKey;
end;

function PathHas(const Paths, Dir: String): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0;
end;

procedure AddToPath(const Dir: String);
var
  Paths: String;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Paths) then Paths := '';
  if PathHas(Paths, Dir) then Exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then Paths := Paths + ';';
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Paths + Dir);
end;

procedure RemoveFromPath(const Dir: String);
var
  Paths, Rest, Part, Kept: String;
  P: Integer;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Paths) then Exit;
  Rest := Paths + ';';
  Kept := '';
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    Part := Copy(Rest, 1, P - 1);
    Rest := Copy(Rest, P + 1, Length(Rest));
    if (Part <> '') and (CompareText(Part, Dir) <> 0) then
    begin
      if Kept <> '' then Kept := Kept + ';';
      Kept := Kept + Part;
    end;
  end;
  if Kept <> Paths then RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Kept);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}\bin'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}\bin'));
end;
