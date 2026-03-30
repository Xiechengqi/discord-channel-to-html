interface Message {
  id: number;
  author: string;
  timestamp: string;
  content: string;
}

interface MessageItemProps {
  author: string;
  timestamp: string;
  messages: Message[];
}

// Deterministic color from author name
const AUTHOR_COLORS = [
  '#d03d33', '#b5651d', '#9b7b0e', '#2d8b39',
  '#206694', '#7b3fa0', '#c44d8e', '#1a8b6e',
];

function authorColor(name: string): string {
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = name.charCodeAt(i) + ((hash << 5) - hash);
  }
  return AUTHOR_COLORS[Math.abs(hash) % AUTHOR_COLORS.length];
}

function formatTimestamp(ts: string): string {
  if (!ts) return '';
  try {
    const d = new Date(ts);
    if (isNaN(d.getTime())) return ts;
    const now = new Date();
    const isToday = d.toDateString() === now.toDateString();
    const time = d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
    if (isToday) return `Today at ${time}`;
    return `${d.toLocaleDateString(undefined, { month: '2-digit', day: '2-digit', year: 'numeric' })} ${time}`;
  } catch {
    return ts;
  }
}

export function MessageItem({ author, timestamp, messages }: MessageItemProps) {
  const color = authorColor(author);

  return (
    <div className="group hover:bg-discord-hover px-4 py-0.5 mt-4 first:mt-0">
      {/* Header with author + timestamp */}
      <div className="flex items-baseline gap-2">
        <span className="font-medium text-base leading-snug" style={{ color }}>
          {author || 'Unknown'}
        </span>
        <span className="text-discord-text-muted text-xs">
          {formatTimestamp(timestamp)}
        </span>
      </div>

      {/* Message contents */}
      {messages.map((msg) => (
        <div
          key={msg.id}
          className="text-discord-text text-[0.9375rem] leading-[1.375rem] py-0.5 break-words whitespace-pre-wrap"
        >
          {msg.content}
        </div>
      ))}
    </div>
  );
}
