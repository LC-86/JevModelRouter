import type { DebugResult } from './bridge';

export type DebugFile = { name: string; size: number; text?: string; data?: string };

export type DebugMessage = {
  id: string;
  role: 'user' | 'assistant';
  status: 'pending' | 'success' | 'error';
  content: string;
  images?: string[];
  files?: DebugFile[];
  warning?: string;
  reasoning?: boolean;
  raw?: string;
  result?: DebugResult;
  request?: { target: string; endpoint: string; parameters: Record<string, unknown> };
};

export type DebugContent = string | Record<string, unknown>[];

export function debugContent(text: string, images: string[] = [], endpoint = 'chat/completions', files: DebugFile[] = []): DebugContent {
  const documents = files.filter(file => file.data);
  text = [text, ...files.filter(file => file.text !== undefined).map(file => `File: ${file.name}\n${file.text}`)].filter(Boolean).join('\n\n');
  if (!images.length && !files.length) return text;
  return [
    ...(text ? [{ type: endpoint === 'responses' ? 'input_text' : 'text', text }] : []),
    ...documents.map(file => {
      if (endpoint === 'responses') return { type: 'input_file', filename: file.name, file_data: file.data };
      if (endpoint === 'messages') return { type: 'document', title: file.name, source: { type: 'base64', media_type: 'application/pdf', data: file.data!.split(',')[1] } };
      return { type: 'file', file: { filename: file.name, file_data: file.data } };
    }),
    ...images.map(url => {
      if (endpoint === 'responses') return { type: 'input_image', image_url: url };
      if (endpoint === 'messages') {
        const [header, data] = url.split(',');
        return { type: 'image', source: { type: 'base64', media_type: header.slice(5, -7), data } };
      }
      return { type: 'image_url', image_url: { url } };
    }),
  ];
}

export function debugHistory(messages: DebugMessage[], endpoint = 'chat/completions') {
  return messages.filter(message => message.status === 'success')
    .map(({ role, content, images, files }) => ({ role, content: debugContent(content, images, endpoint, files) }));
}

// Text only: tool arguments and reasoning blocks are available in the raw JSON.
export function debugResponseText(data: unknown): string {
  if (!data || typeof data !== 'object') return '';
  const value = data as Record<string, any>;
  const textParts = (parts: unknown): string => typeof parts === 'string' ? parts :
    Array.isArray(parts) ? parts.filter(part => part && ['text', 'output_text'].includes(part.type) && typeof part.text === 'string').map(part => part.text).join('\n\n') : '';
  const chat = value.choices?.[0]?.message;
  if (chat) return textParts(chat.content) || (typeof chat.refusal === 'string' ? chat.refusal : '');
  if (Array.isArray(value.content)) return textParts(value.content);
  if (Array.isArray(value.output)) return value.output.filter(item => item?.type === 'message').map(item => textParts(item.content)).filter(Boolean).join('\n\n');
  return '';
}

// Mirror the local gateway payload so it can be inspected while awaiting a reply.
export function debugRequestBody(target: string, endpoint: string, prompt: DebugContent, history: { role: string; content: DebugContent }[], parameters: Record<string, unknown>) {
  const responses = endpoint === 'responses';
  const body: Record<string, unknown> = {
    model: target,
    [responses ? 'input' : 'messages']: [...history, { role: 'user', content: prompt }],
  };
  for (const key of Object.keys(parameters).sort()) {
    const normalized = key === 'max_tokens' && responses ? 'max_output_tokens' : key === 'max_output_tokens' && !responses ? 'max_tokens' : key;
    body[normalized] = parameters[key];
  }
  return body;
}

export function parseDebugParameters(rows: { key: string; value: string }[]): Record<string, unknown> {
  const params: Record<string, unknown> = Object.create(null);
  for (const row of rows) {
    const key = row.key.trim();
    if (!key || Object.hasOwn(params, key)) throw new Error('Invalid or duplicate parameter');
    try { params[key] = JSON.parse(row.value); }
    catch { params[key] = row.value; }
  }
  return params;
}

export function debugResponseWarning(data: unknown): string | undefined {
  if (!data || typeof data !== 'object') return undefined;
  const value = data as Record<string, any>;
  const limited = value.choices?.[0]?.finish_reason === 'length' || value.stop_reason === 'max_tokens' || value.incomplete_details?.reason === 'max_output_tokens';
  if (!limited) return undefined;
  return debugResponseText(data) ? 'Output limit reached; the answer is incomplete.' :
    'Output limit reached before a final answer. Increase the output limit or reduce reasoning using parameters supported by this provider.';
}
