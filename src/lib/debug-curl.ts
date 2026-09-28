export function makeCurl(port: number, endpoint: string, body: Record<string, unknown>): string {
  const quote = (value: string) => "'" + value.replace(/'/g, "'\\''") + "'";
  return `curl -N -X POST \\\n  --url 'http://127.0.0.1:${port}/v1/${endpoint}' \\\n  -H 'Content-Type: application/json' \\\n  --data-raw ${quote(JSON.stringify(body, null, 2))}`;
}

// Parse a request description, never execute shell code or expand variables/files.
export function parseCurl(command: string, port: number) {
  const words: string[] = [];
  let word = '', quote = '', started = false;
  for (let i = 0; i < command.length; i++) {
    const char = command[i];
    if (char === '\\' && quote !== "'") {
      const next = command[++i];
      if (next === undefined) throw new Error('Incomplete cURL escape');
      if (next !== '\n') { word += quote === '"' && !['\\', '$', '`', '"'].includes(next) ? '\\' + next : next; started = true; }
    } else if (quote) {
      if (char === quote) quote = ''; else word += char;
    } else if (char === "'" || char === '"') { quote = char; started = true; }
    else if (/\s/.test(char)) { if (started) words.push(word); word = ''; started = false; }
    else { if (/[;|&<>`]/.test(char)) throw new Error('Shell operators are not supported'); word += char; started = true; }
  }
  if (quote) throw new Error('Unclosed cURL quote');
  if (started) words.push(word);
  if (words.shift() !== 'curl') throw new Error('Start the request with curl');
  let url = '', data: string | undefined, method = 'POST';
  const headers: Record<string, string> = {};
  while (words.length) {
    const flag = words.shift()!;
    const value = () => { const next = words.shift(); if (next === undefined) throw new Error(`Missing value: ${flag}`); return next; };
    if (['-N', '--no-buffer', '-s', '-S', '-sS', '-i', '--include'].includes(flag)) continue;
    if (flag === '-X' || flag === '--request') method = value().toUpperCase();
    else if (flag === '-H' || flag === '--header') {
      const header = value(), colon = header.indexOf(':');
      if (colon < 1) throw new Error('Invalid cURL header');
      headers[header.slice(0, colon).trim()] = header.slice(colon + 1).trim();
    } else if (['-d', '--data', '--data-raw', '--data-binary', '--json'].includes(flag)) {
      if (data !== undefined) throw new Error('Use one JSON request body');
      data = value();
    } else if (flag === '--url') { if (url) throw new Error('Use one request URL'); url = value(); }
    else if (!flag.startsWith('-') && !url) url = flag;
    else throw new Error(`Unsupported cURL argument: ${flag}`);
  }
  const parsed = new URL(url);
  if (parsed.protocol !== 'http:' || !['127.0.0.1', 'localhost'].includes(parsed.hostname) || parsed.port !== String(port) || parsed.search || parsed.hash || parsed.username || parsed.password) throw new Error('Use the current local AutoJev gateway URL');
  const endpoint = parsed.pathname.replace(/^\/v1\//, '');
  if (!['chat/completions', 'responses', 'messages'].includes(endpoint) || method !== 'POST') throw new Error('Use POST with a supported model API endpoint');
  const body: unknown = JSON.parse(data ?? '');
  if (!body || typeof body !== 'object' || Array.isArray(body)) throw new Error('Request body must be a JSON object');
  return { endpoint, headers, body: body as Record<string, unknown> };
}
