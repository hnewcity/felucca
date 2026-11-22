const shellQuote = (value) => `'${String(value).replaceAll("'", "'\\''")}'`;
const powershellQuote = (value) => `'${String(value).replaceAll("'", "''")}'`;

// The complete connection is encoded as data. It is never interpolated into
// executable script text, including user-selected paths and visitor entries.
export function connectionCommand(config, origin, system = "unix", locale = "en") {
  const url = new URL(origin);
  if (
    !/^https?:$/.test(url.protocol) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    url.pathname !== "/"
  )
    throw new Error("flow.hostInvalid");
  if (!/^(en|zh|ja|ko|fr|es|ru)$/.test(locale)) throw new Error("flow.hostInvalid");
  const bytes = new TextEncoder().encode(JSON.stringify(config));
  const encoded = btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(""));
  const args = [url.origin, config.key_id, config.vkey, encoded, locale];
  if (system === "windows") {
    const [server, device, token, value, language] = args.map(powershellQuote);
    return `& ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing -Headers @{'Accept-Language'=${language}} ${powershellQuote(`${url.origin}/connect.ps1`)}).Content)) -Server ${server} -Device ${device} -Token ${token} -Config ${value} -Language ${language}`;
  }
  return `curl -fsS --connect-timeout 15 --retry 3 -H ${shellQuote(`Accept-Language: ${locale}`)} ${shellQuote(`${url.origin}/connect.sh`)} | sh -s -- ${args.map(shellQuote).join(" ")}`;
}
