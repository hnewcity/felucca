const encoder = new TextEncoder();
const crcTable = Array.from({ length: 256 }, (_, index) => {
  let value = index;
  for (let bit = 0; bit < 8; bit++) value = (value >>> 1) ^ (value & 1 ? 0xedb88320 : 0);
  return value >>> 0;
});
function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = crcTable[(crc ^ byte) & 255] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

// STORE entries keep the binary byte-for-byte intact. Unix executable bits
// make macOS .command files runnable after the system's normal ZIP extraction.
export function zipFiles(files) {
  const parts = [],
    directory = [];
  let offset = 0,
    directoryBytes = 0;
  for (const { name, content, executable = false } of files) {
    const filename = encoder.encode(name);
    const bytes = typeof content === "string" ? encoder.encode(content) : content;
    const crc = crc32(bytes);
    const local = new Uint8Array(30 + filename.length);
    const view = new DataView(local.buffer);
    view.setUint32(0, 0x04034b50, true);
    view.setUint16(4, 20, true);
    view.setUint16(6, 0x800, true);
    view.setUint16(12, 33, true);
    view.setUint32(14, crc, true);
    view.setUint32(18, bytes.length, true);
    view.setUint32(22, bytes.length, true);
    view.setUint16(26, filename.length, true);
    local.set(filename, 30);
    parts.push(local, bytes);
    const central = new Uint8Array(46 + filename.length);
    const index = new DataView(central.buffer);
    index.setUint32(0, 0x02014b50, true);
    index.setUint16(4, 0x0314, true);
    central.set(local.subarray(4, 28), 6);
    index.setUint32(38, (executable ? 0o100700 : 0o100600) << 16, true);
    index.setUint32(42, offset, true);
    central.set(filename, 46);
    directory.push(central);
    directoryBytes += central.length;
    offset += local.length + bytes.length;
  }
  const end = new Uint8Array(22);
  const view = new DataView(end.buffer);
  view.setUint32(0, 0x06054b50, true);
  view.setUint16(8, files.length, true);
  view.setUint16(10, files.length, true);
  view.setUint32(12, directoryBytes, true);
  view.setUint32(16, offset, true);
  return new Blob([...parts, ...directory, end], { type: "application/zip" });
}

export const packageTargets = {
  "aarch64-apple-darwin": "flow.macArm",
  "x86_64-apple-darwin": "flow.macIntel",
  "aarch64-unknown-linux-gnu": "flow.linuxArm",
  "x86_64-unknown-linux-gnu": "flow.linuxIntel",
  "aarch64-pc-windows-msvc": "flow.windowsArm",
  "x86_64-pc-windows-msvc": "flow.windowsIntel",
};
export function preferredTarget(packages, platform = navigator.platform) {
  const family = /mac/i.test(platform) ? "apple" : /win/i.test(platform) ? "windows" : "linux";
  return packages.find((item) => item.target.includes(family))?.target ?? "";
}
export function packageScripts(target) {
  return target.includes("windows")
    ? { connect: "Connect.cmd", install: "Install.cmd" }
    : target.includes("apple")
      ? { connect: "Connect.command", install: "Install.command" }
      : { connect: "connect.sh", install: "install.sh" };
}
const quote = (text) => `'${String(text).replaceAll("'", "'\\''")}'`;

export function connectionPackage({ config, binary, target, id, name, locale, t }) {
  // These identifiers become service/shortcut names, never user-supplied labels.
  if (!/^[a-z0-9-]{1,64}$/.test(id) || !/^(zh|en|ja|ko|fr|es|ru)$/.test(locale))
    throw new Error("Invalid package identity");
  const windows = target.includes("windows");
  const scripts = packageScripts(target);
  const binaryName = windows ? "felucca-agent.exe" : "felucca-agent";
  const startText = t("flow.packageStarting", { name });
  const installedText = t("flow.packageInstalled");
  const alreadyText = t("flow.packageExists");
  const shellStart = `#!/bin/sh
set -eu
umask 077
cd -- "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
chmod 600 connection.json
chmod 700 felucca-agent
printf '%s\\n' ${quote(startText)}
exec ./felucca-agent --config ./connection.json --lang ${quote(locale)}
`;
  const shellInstall = `#!/bin/sh
set -eu
umask 077
cd -- "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
felucca_base="\${XDG_DATA_HOME:-\${HOME:?}/.local/share}/felucca"
felucca_folder="$felucca_base/${id}"
if [ -e "$felucca_folder" ]; then
  printf '%s\\n' ${quote(alreadyText)}
  exit 1
fi
mkdir -p "$felucca_base"
mkdir "$felucca_folder"
cp felucca-agent connection.json "$felucca_folder/"
chmod 700 "$felucca_folder/felucca-agent"
chmod 600 "$felucca_folder/connection.json"
"$felucca_folder/felucca-agent" service --name ${quote(id)} --scope user install --config "$felucca_folder/connection.json" --workdir "$felucca_folder" -- --lang ${quote(locale)}
"$felucca_folder/felucca-agent" service --name ${quote(id)} --scope user start
printf '%s\\n' ${quote(installedText)}
`;
  const cmdStart = `@echo off
setlocal DisableDelayedExpansion
cd /d "%~dp0"
"%~dp0felucca-agent.exe" --config "%~dp0connection.json" --lang ${locale}
pause
`;
  // A per-user startup shortcut needs no administrator account and does not
  // grant a writable user binary system-service privileges.
  const cmdInstall = `@echo off
setlocal DisableDelayedExpansion
set "FELUCCA_PACKAGE_DIR=%LOCALAPPDATA%\\Felucca\\${id}"
if exist "%FELUCCA_PACKAGE_DIR%" exit /b 1
mkdir "%FELUCCA_PACKAGE_DIR%" || exit /b 1
copy /b "%~dp0felucca-agent.exe" "%FELUCCA_PACKAGE_DIR%\\felucca-agent.exe" >nul || exit /b 1
copy /b "%~dp0connection.json" "%FELUCCA_PACKAGE_DIR%\\connection.json" >nul || exit /b 1
powershell -NoProfile -Command "$w = New-Object -ComObject WScript.Shell; $s = $w.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Startup')) 'Felucca-${id}.lnk')); $s.TargetPath = Join-Path $env:FELUCCA_PACKAGE_DIR 'felucca-agent.exe'; $s.Arguments = '--config ' + [char]34 + (Join-Path $env:FELUCCA_PACKAGE_DIR 'connection.json') + [char]34 + ' --lang ${locale}'; $s.WorkingDirectory = $env:FELUCCA_PACKAGE_DIR; $s.WindowStyle = 7; $s.Save(); Start-Process -FilePath $s.TargetPath -ArgumentList $s.Arguments -WorkingDirectory $s.WorkingDirectory -WindowStyle Minimized"
pause
`;
  const readme =
    [
      `Felucca · ${name}`,
      t(target.includes("linux") ? "flow.packageLinuxInstructions" : "flow.packageInstructions", {
        connect: target.includes("linux") ? `sh ${scripts.connect}` : scripts.connect,
      }),
      t("flow.packageBackground", { install: scripts.install }),
      t("flow.packageSecret"),
    ].join("\n\n") + "\n";
  return zipFiles([
    { name: `${id}/${binaryName}`, content: binary, executable: true },
    { name: `${id}/connection.json`, content: JSON.stringify(config, null, 2) + "\n" },
    {
      name: `${id}/${scripts.connect}`,
      content: windows ? cmdStart.replaceAll("\n", "\r\n") : shellStart,
      executable: !windows,
    },
    {
      name: `${id}/${scripts.install}`,
      content: windows ? cmdInstall.replaceAll("\n", "\r\n") : shellInstall,
      executable: !windows,
    },
    { name: `${id}/README.txt`, content: readme },
  ]);
}
