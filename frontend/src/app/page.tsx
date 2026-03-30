'use client';

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ChatHeader } from '@/components/chat-header';
import { MessageList } from '@/components/message-list';

interface Message {
  id: number;
  author: string;
  timestamp: string;
  content: string;
}

interface HealthInfo {
  ok: boolean;
  message_count: number;
  uptime_secs: number;
  channel: string;
  monitor_status: string;
}

const PAGE_SIZE = 50;
const INITIAL_SIZE = 100;
const LOAD_MORE_THRESHOLD = 120; // px from top to trigger load-more

export default function Home() {
  const [messages, setMessages] = useState<Message[]>([]);
  const [health, setHealth] = useState<HealthInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(true);
  const [isLoadingMore, setIsLoadingMore] = useState(false);
  const [resyncing, setResyncing] = useState(false);

  const scrollRef = useRef<HTMLDivElement>(null);
  // When prepending messages, save the old scrollHeight here so we can restore
  // scroll position in useLayoutEffect before the browser paints.
  const pendingScrollRestore = useRef<number | null>(null);
  const shouldAutoScroll = useRef(true);

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
    if (isLoadingMore || !hasMore || messages.length === 0) return;
    setIsLoadingMore(true);

    const oldestId = messages[0].id;
    // Save current scrollHeight before React re-renders with new messages
    pendingScrollRestore.current = scrollRef.current?.scrollHeight ?? 0;

    try {
      const res = await fetch(`/api/messages?before_id=${oldestId}&limit=${PAGE_SIZE}`);
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
  }, [isLoadingMore, hasMore, messages]);

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
    try {
      const n = initial ? INITIAL_SIZE : PAGE_SIZE;
      const res = await fetch(`/api/messages/latest?n=${n}`);
      const data = await res.json();
      if (!data.ok || !data.messages) return;

      setMessages(prev => {
        if (initial) return data.messages;
        // During resync, messages were cleared — treat first poll result as initial
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
  }, []);

  const fetchHealth = useCallback(async () => {
    try {
      const res = await fetch('/health');
      const data = await res.json();
      if (data.ok) {
        setHealth(data);
        // Clear resyncing state once monitor is back to monitoring
        if (data.monitor_status === 'monitoring') {
          setResyncing(false);
        }
      }
    } catch {
      // ignore
    }
  }, []);

  const handleResync = useCallback(async () => {
    setResyncing(true);
    setMessages([]);
    setHasMore(true);
    setLoading(true);
    try {
      await fetch('/api/resync', { method: 'POST' });
    } catch {
      // ignore — poll will eventually reflect new state
    }
  }, []);

  useEffect(() => {
    fetchLatest(true);
    fetchHealth();
    const msgInterval = setInterval(() => fetchLatest(false), 5000);
    const healthInterval = setInterval(fetchHealth, 10000);
    return () => {
      clearInterval(msgInterval);
      clearInterval(healthInterval);
    };
  }, [fetchLatest, fetchHealth]);

  // ── Auto-scroll to bottom on new messages (only when pinned) ──
  useLayoutEffect(() => {
    if (shouldAutoScroll.current && scrollRef.current && pendingScrollRestore.current === null) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages]);

  return (
    <div className="flex flex-col h-screen">
      <ChatHeader
        channel={health?.channel || 'Loading...'}
        messageCount={health?.message_count || 0}
        connected={health?.ok || false}
        monitorStatus={health?.monitor_status}
        onResync={handleResync}
        resyncing={resyncing}
      />
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto"
        onScroll={handleScroll}
      >
        {/* Load-more indicator at the very top */}
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

        {loading ? (
          <div className="flex items-center justify-center h-32 text-discord-text-muted">
            Loading messages...
          </div>
        ) : messages.length === 0 ? (
          <div className="flex items-center justify-center h-32 text-discord-text-muted">
            No messages yet
          </div>
        ) : (
          <MessageList messages={messages} />
        )}
      </div>
    </div>
  );
}
