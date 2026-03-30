'use client';

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import Link from 'next/link';
import { ChatHeader } from '@/components/chat-header';
import { MessageList } from '@/components/message-list';

interface Message {
  id: number;
  author: string;
  timestamp: string;
  content: string;
}

interface Channel {
  channel_id: string;
  name: string;
  monitored: boolean;
  message_count: number;
}

interface HealthInfo {
  ok: boolean;
  total_messages: number;
  uptime_secs: number;
  monitored_channels: number;
}

const PAGE_SIZE = 50;
const INITIAL_SIZE = 100;
const LOAD_MORE_THRESHOLD = 120;

export default function Home() {
  const [channels, setChannels] = useState<Channel[]>([]);
  const [selectedChannel, setSelectedChannel] = useState<string>('');
  const [messages, setMessages] = useState<Message[]>([]);
  const [health, setHealth] = useState<HealthInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(true);
  const [isLoadingMore, setIsLoadingMore] = useState(false);

  const scrollRef = useRef<HTMLDivElement>(null);
  const pendingScrollRestore = useRef<number | null>(null);
  const shouldAutoScroll = useRef(true);

  // Fetch channels list
  const fetchChannels = useCallback(async () => {
    try {
      const res = await fetch('/api/channels');
      const data = await res.json();
      if (data.ok) {
        const monitored = data.channels.filter((c: Channel) => c.monitored);
        setChannels(monitored);
        if (!selectedChannel && monitored.length > 0) {
          setSelectedChannel(monitored[0].channel_id);
        }
      }
    } catch (e) {
      console.error('Failed to fetch channels:', e);
    }
  }, [selectedChannel]);

  // Reset messages when channel changes
  useEffect(() => {
    if (selectedChannel) {
      setMessages([]);
      setHasMore(true);
      setLoading(true);
    }
  }, [selectedChannel]);

  // ── Restore scroll position after prepend (runs before browser paint) ──
  useLayoutEffect(() => {
    if (pendingScrollRestore.current !== null && scrollRef.current) {
      const el = scrollRef.current;
      el.scrollTop = el.scrollHeight - pendingScrollRestore.current;
      pendingScrollRestore.current = null;
    }
  }, [messages]);

  // ── Load older messages (infinite scroll upward) ──
  const loadMore = useCallback(async () => {
    if (isLoadingMore || !hasMore || messages.length === 0 || !selectedChannel) return;
    setIsLoadingMore(true);

    const oldestId = messages[0].id;
    pendingScrollRestore.current = scrollRef.current?.scrollHeight ?? 0;

    try {
      const res = await fetch(`/api/messages?channel_id=${selectedChannel}&before_id=${oldestId}&limit=${PAGE_SIZE}`);
      const data = await res.json();
      if (!data.ok) return;

      const older: Message[] = data.messages;
      if (older.length === 0) {
        setHasMore(false);
        pendingScrollRestore.current = null;
        return;
      }

      setMessages(prev => [...older, ...prev]);
      if (!data.has_more) setHasMore(false);
    } catch {
      pendingScrollRestore.current = null;
    } finally {
      setIsLoadingMore(false);
    }
  }, [isLoadingMore, hasMore, messages, selectedChannel]);

  // ── Scroll handler: trigger load-more near top, track auto-scroll ──
  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;

    // Auto-scroll: stay pinned to bottom only when already near bottom
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 100;
    shouldAutoScroll.current = atBottom;

    // Load more when scrolled near the top
    if (el.scrollTop < LOAD_MORE_THRESHOLD && !isLoadingMore && hasMore) {
      loadMore();
    }
  }, [isLoadingMore, hasMore, loadMore]);

  // ── Initial load ──
  const fetchLatest = useCallback(async (initial: boolean) => {
    if (!selectedChannel) return;
    try {
      const n = initial ? INITIAL_SIZE : PAGE_SIZE;
      const res = await fetch(`/api/messages/latest?channel_id=${selectedChannel}&n=${n}`);
      const data = await res.json();
      if (!data.ok || !data.messages) return;

      setMessages(prev => {
        if (initial) return data.messages;
        if (prev.length === 0 && data.messages.length > 0) return data.messages;
        const existingIds = new Set(prev.map((m: Message) => m.id));
        const newMsgs = data.messages.filter((m: Message) => !existingIds.has(m.id));
        if (newMsgs.length === 0) return prev;
        return [...prev, ...newMsgs];
      });
      setLoading(false);
    } catch {
      // retry on next poll
    } finally {
      if (initial) setLoading(false);
    }
  }, [selectedChannel]);

  const fetchHealth = useCallback(async () => {
    try {
      const res = await fetch('/health');
      const data = await res.json();
      if (data.ok) setHealth(data);
    } catch {
      // ignore
    }
  }, []);

  useEffect(() => {
    fetchChannels();
    fetchHealth();
    const channelInterval = setInterval(fetchChannels, 10000);
    const healthInterval = setInterval(fetchHealth, 10000);
    return () => {
      clearInterval(channelInterval);
      clearInterval(healthInterval);
    };
  }, [fetchChannels, fetchHealth]);

  useEffect(() => {
    if (!selectedChannel) return;
    fetchLatest(true);
    const msgInterval = setInterval(() => fetchLatest(false), 5000);
    return () => clearInterval(msgInterval);
  }, [selectedChannel, fetchLatest]);

  // ── Auto-scroll to bottom on new messages (only when pinned) ──
  useLayoutEffect(() => {
    if (shouldAutoScroll.current && scrollRef.current && pendingScrollRestore.current === null) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages]);

  return (
    <div className="flex flex-col h-screen">
      <div className="bg-discord-bg-secondary border-b border-discord-bg-modifier p-3 flex items-center justify-between">
        <div className="flex items-center gap-4">
          <select
            value={selectedChannel}
            onChange={(e) => setSelectedChannel(e.target.value)}
            className="bg-discord-bg-tertiary text-discord-text px-3 py-2 rounded border-none outline-none"
          >
            {channels.length === 0 && <option value="">No monitored channels</option>}
            {channels.map(ch => (
              <option key={ch.channel_id} value={ch.channel_id}>
                # {ch.name} ({ch.message_count})
              </option>
            ))}
          </select>
          <Link
            href="/channels"
            className="px-3 py-2 bg-discord-blurple hover:bg-discord-blurple-dark rounded text-sm transition-colors"
          >
            Manage Channels
          </Link>
        </div>
        <div className="text-sm text-discord-text-muted">
          {health?.monitored_channels || 0} monitored · {health?.total_messages || 0} total messages
        </div>
      </div>
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto"
        onScroll={handleScroll}
      >
        {!selectedChannel ? (
          <div className="flex items-center justify-center h-32 text-discord-text-muted">
            No channel selected
          </div>
        ) : loading ? (
          <div className="flex items-center justify-center h-32 text-discord-text-muted">
            Loading messages...
          </div>
        ) : (
          <>
            {hasMore && (
              <div className="flex justify-center py-3 text-discord-text-muted text-xs">
                {isLoadingMore ? 'Loading...' : (
                  <button
                    onClick={loadMore}
                    className="px-3 py-1 rounded bg-discord-bg-secondary hover:bg-discord-bg-tertiary transition-colors cursor-pointer"
                  >
                    Load older messages
                  </button>
                )}
              </div>
            )}
            {!hasMore && messages.length > 0 && (
              <div className="flex justify-center py-3 text-discord-text-muted text-xs">
                Beginning of channel history
              </div>
            )}
            {messages.length === 0 ? (
              <div className="flex items-center justify-center h-32 text-discord-text-muted">
                No messages yet
              </div>
            ) : (
              <MessageList messages={messages} />
            )}
          </>
        )}
      </div>
    </div>
  );
}
