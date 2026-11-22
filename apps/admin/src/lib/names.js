const alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
const normalize = (value) => value.trim().toLowerCase();

// Name suggestions are never authentication secrets.
export function generateName(prefix, { existingNames = [], current = "" } = {}) {
  const used = new Set([...existingNames, current].filter(Boolean).map(normalize));
  const suffix = Array.from(
    crypto.getRandomValues(new Uint8Array(5)),
    (byte) => alphabet[byte % alphabet.length],
  ).join("");
  const base = `${prefix}-${suffix}`;
  let candidate = base;
  for (let index = 2; used.has(normalize(candidate)); index += 1) candidate = `${base}-${index}`;
  return candidate;
}
