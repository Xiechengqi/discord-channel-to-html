interface ChatHeaderProps {
  channel: string;
  messageCount: number;
  connected: boolean;
  monitorStatus?: string;
  onResync?: () => void;
  resyncing?: boolean;
}

export function ChatHeader({ channel, messageCount, connected, monitorStatus, onResync, resyncing }: ChatHeaderProps) {
  const isResyncing = resyncing || monitorStatus === 'resyncing' || monitorStatus === 'navigating' || monitorStatus === 'loading_history';

  return (
    <div className="flex items-center gap-3 px-4 h-12 bg-discord-bg border-b border-discord-divider shrink-0">
      <span className="text-discord-text-muted text-xl">#</span>
      <h1 className="text-discord-header font-semibold text-base">{channel}</h1>
      <div className="h-6 w-px bg-discord-divider mx-1" />
      <span className="text-discord-text-muted text-sm">
        {messageCount.toLocaleString()} messages
      </span>
      <div className="ml-auto flex items-center gap-3">
        {onResync && (
          <button
            onClick={onResync}
            disabled={isResyncing}
            className="px-3 py-1 rounded text-xs bg-discord-bg-secondary border border-discord-divider hover:bg-discord-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors text-discord-text"
          >
            {isResyncing ? (monitorStatus === 'loading_history' ? 'Loading history...' : monitorStatus === 'navigating' ? 'Navigating...' : 'Resyncing...') : 'Re-sync'}
          </button>
        )}
        <div className="flex items-center gap-2">
          <div
            className={`w-2 h-2 rounded-full ${connected ? 'bg-green-500' : 'bg-red-500'}`}
          />
          <span className="text-discord-text-muted text-xs">
            {connected ? 'Live' : 'Disconnected'}
          </span>
        </div>
      </div>
    </div>
  );
}
