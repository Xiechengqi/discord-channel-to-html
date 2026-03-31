'use client';

import { useEffect, useState } from 'react';
import Link from 'next/link';

interface Channel {
  channel_id: string;
  name: string;
  type: string;
  channel_url: string;
  monitored: boolean;
  message_count: number;
}

interface Config {
  poll_interval_secs: number;
  max_history_pages: number | null;
}

export default function ChannelsPage() {
  const [channels, setChannels] = useState<Channel[]>([]);
  const [config, setConfig] = useState<Config | null>(null);
  const [editConfig, setEditConfig] = useState<Config | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);

  const fetchChannels = async () => {
    try {
      const res = await fetch('/api/channels');
      const data = await res.json();
      if (data.ok) setChannels(data.channels);
    } catch (e) {
      console.error('Failed to fetch channels:', e);
    } finally {
      setLoading(false);
    }
  };

  const fetchConfig = async () => {
    try {
      const res = await fetch('/api/config');
      const data = await res.json();
      if (data.ok) {
        setConfig(data);
        setEditConfig(data);
      }
    } catch (e) {
      console.error('Failed to fetch config:', e);
    }
  };

  const updateConfig = async (updates: Partial<Config>) => {
    try {
      await fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(updates),
      });
      await fetchConfig();
    } catch (e) {
      console.error('Failed to update config:', e);
    }
  };

  const handleSaveConfig = async () => {
    if (!editConfig) return;
    await updateConfig({
      poll_interval_secs: editConfig.poll_interval_secs,
      max_history_pages: editConfig.max_history_pages,
    });
  };

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      await fetch('/api/channels/refresh', { method: 'POST' });
      await fetchChannels();
    } catch (e) {
      console.error('Refresh failed:', e);
    } finally {
      setRefreshing(false);
    }
  };

  const toggleMonitor = async (channelId: string, monitored: boolean) => {
    const method = monitored ? 'DELETE' : 'POST';
    try {
      await fetch(`/api/channels/${channelId}/monitor`, { method });
      await fetchChannels();
    } catch (e) {
      console.error('Toggle failed:', e);
    }
  };

  const handleResync = async (channelId: string) => {
    if (!confirm('Clear all data and re-scrape this channel?')) return;
    try {
      await fetch(`/api/channels/${channelId}/resync`, { method: 'POST' });
      await fetchChannels();
    } catch (e) {
      console.error('Resync failed:', e);
    }
  };

  useEffect(() => {
    fetchChannels();
    fetchConfig();
  }, []);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-discord-bg text-discord-text">
        Loading channels...
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-discord-bg text-discord-text p-6">
      <div className="max-w-4xl mx-auto">
        <div className="flex items-center justify-between mb-6">
          <h1 className="text-2xl font-bold">Channel Management</h1>
          <div className="flex gap-3">
            <Link
              href="/"
              className="px-4 py-2 bg-discord-bg-secondary hover:bg-discord-bg-tertiary rounded transition-colors"
            >
              View Messages
            </Link>
            <button
              onClick={handleRefresh}
              disabled={refreshing}
              className="px-4 py-2 bg-discord-blurple hover:bg-discord-blurple-dark rounded transition-colors disabled:opacity-50"
            >
              {refreshing ? 'Refreshing...' : 'Refresh from Discord'}
            </button>
          </div>
        </div>

        {editConfig && (
          <div className="mb-6 p-4 bg-discord-bg-secondary rounded">
            <h2 className="text-lg font-semibold mb-3">Settings</h2>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm text-discord-text-muted mb-1">
                  Poll Interval (seconds)
                </label>
                <input
                  type="number"
                  value={editConfig.poll_interval_secs}
                  onChange={(e) => setEditConfig({ ...editConfig, poll_interval_secs: parseInt(e.target.value) || 1 })}
                  className="w-full bg-discord-bg-tertiary px-3 py-2 rounded border-none outline-none"
                  min="1"
                />
              </div>
              <div>
                <label className="block text-sm text-discord-text-muted mb-1">
                  Max History Pages (empty = unlimited)
                </label>
                <input
                  type="number"
                  value={editConfig.max_history_pages ?? ''}
                  onChange={(e) => setEditConfig({ ...editConfig, max_history_pages: e.target.value ? parseInt(e.target.value) : null })}
                  className="w-full bg-discord-bg-tertiary px-3 py-2 rounded border-none outline-none"
                  placeholder="Unlimited"
                  min="1"
                />
              </div>
            </div>
            <button
              onClick={handleSaveConfig}
              className="mt-4 px-4 py-2 bg-discord-blurple hover:bg-discord-blurple-dark rounded transition-colors"
            >
              Save
            </button>
          </div>
        )}

        <div className="space-y-2">
          {channels.map(ch => (
            <div
              key={ch.channel_id}
              className="flex items-center justify-between p-4 bg-discord-bg-secondary rounded hover:bg-discord-bg-tertiary transition-colors"
            >
              <div className="flex-1">
                <div className="flex items-center gap-3">
                  <span className="font-semibold"># {ch.name}</span>
                  <span className="text-xs text-discord-text-muted">{ch.type}</span>
                  {ch.monitored && (
                    <span className="text-xs px-2 py-0.5 bg-green-600 rounded">Monitored</span>
                  )}
                </div>
                <div className="text-sm text-discord-text-muted mt-1">
                  {ch.message_count} messages
                </div>
              </div>
              <div className="flex gap-2">
                <button
                  onClick={() => toggleMonitor(ch.channel_id, ch.monitored)}
                  className={`px-3 py-1 rounded text-sm transition-colors ${
                    ch.monitored
                      ? 'bg-red-600 hover:bg-red-700'
                      : 'bg-green-600 hover:bg-green-700'
                  }`}
                >
                  {ch.monitored ? 'Stop' : 'Monitor'}
                </button>
                {ch.monitored && (
                  <button
                    onClick={() => handleResync(ch.channel_id)}
                    className="px-3 py-1 bg-discord-bg-tertiary hover:bg-discord-bg-modifier rounded text-sm transition-colors"
                  >
                    Resync
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
