import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

export function DebugMarkdown({ children }: { children: string }) {
  return <Markdown remarkPlugins={[remarkGfm]} skipHtml components={{
    a: ({ children, href }) => <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>,
    // Do not load remote tracking images contained in a model response.
    img: ({ alt }) => <span>{alt}</span>,
  }}>{children}</Markdown>;
}
