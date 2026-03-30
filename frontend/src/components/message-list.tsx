import { MessageItem } from './message-item';

interface Message {
  id: number;
  author: string;
  timestamp: string;
  content: string;
}

interface MessageListProps {
  messages: Message[];
}

export function MessageList({ messages }: MessageListProps) {
  // Group consecutive messages by the same author
  const groups: { author: string; timestamp: string; messages: Message[] }[] = [];

  for (const msg of messages) {
    const last = groups[groups.length - 1];
    if (last && last.author === msg.author) {
      last.messages.push(msg);
    } else {
      groups.push({
        author: msg.author,
        timestamp: msg.timestamp,
        messages: [msg],
      });
    }
  }

  return (
    <div className="py-4">
      {groups.map((group) => (
        <MessageItem
          key={group.messages[0].id}
          author={group.author}
          timestamp={group.timestamp}
          messages={group.messages}
        />
      ))}
    </div>
  );
}
