use std::collections::HashSet;

use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info};

use crate::agent_browser::client::AgentBrowserClient;
use crate::db::ScrapedMessage;
use crate::errors::{AppError, AppResult};

#[derive(Debug, Deserialize)]
struct ScrollInfo {
    scroll_height: f64,
    client_height: f64,
    scroll_top: f64,
}

/// Collects all visible messages, including image/video placeholders for media attachments.
///
/// For each message node we:
/// 1. Extract text content from the message-content element.
/// 2. Count images (`[class*="imageWrapper"]`) and videos (`video` elements) that are
///    siblings/descendants of the message container but NOT inside the reply-reference block.
/// 3. Append "[图片]" / "[视频]" placeholders so media-only messages are not stored as empty,
///    and mixed messages don't silently lose the fact that media was present.
///
/// Reply references are intentionally ignored — the replied-to text is excluded so it
/// doesn't pollute the actual message content.
const COLLECT_SCRIPT: &str = r#"JSON.stringify((function() {
    var results = [];
    var msgNodes = document.querySelectorAll('[class*="message"]');
    var idx = 0;
    msgNodes.forEach(function(node) {
        var authorEl = node.querySelector('[class*="username"], [class*="headerText"] span, [class*="author"]');
        var timeEl = node.querySelector('time, [class*="timestamp"]');
        var contentEl = node.querySelector('[id^="message-content-"], [class*="messageContent"]');

        // contentEl is our anchor: Discord always renders it even for media-only messages.
        // Without it we're likely hitting a non-message UI node (date separator, etc.) — skip.
        if (!contentEl) return;

        var msgId = '';
        var idAttr = contentEl.getAttribute('id') || '';
        if (idAttr.startsWith('message-content-')) {
            msgId = idAttr.replace('message-content-', '');
        }

        // Text content — strip the reply-reference block first so its text doesn't bleed in.
        var replyRef = node.querySelector('[class*="repliedMessage"], [class*="replyContext"]');
        var textContent = '';
        if (replyRef) {
            // Clone node, remove the reply block, then read textContent
            var clone = contentEl.cloneNode(true);
            var cloneReply = clone.querySelector('[class*="repliedMessage"], [class*="replyContext"]');
            if (cloneReply) cloneReply.remove();
            textContent = clone.textContent.trim();
        } else {
            textContent = (contentEl.textContent || '').trim();
        }

        // Media detection: look in the message container, excluding the reply-reference subtree.
        // We count unique image wrappers and video elements.
        var imgCount = 0;
        var videoCount = 0;
        var imageWrappers = node.querySelectorAll('[class*="imageWrapper"], [class*="attachedFiles"] img');
        imageWrappers.forEach(function(el) {
            // Exclude anything inside the reply reference
            if (replyRef && replyRef.contains(el)) return;
            imgCount++;
        });
        var videoEls = node.querySelectorAll('video');
        videoEls.forEach(function(el) {
            if (replyRef && replyRef.contains(el)) return;
            videoCount++;
        });

        // Build combined content
        var parts = [];
        if (textContent) parts.push(textContent);
        for (var i = 0; i < imgCount; i++) parts.push('[图片]');
        for (var i = 0; i < videoCount; i++) parts.push('[视频]');
        var combined = parts.join(' ');

        // Skip nodes that produced nothing (true UI noise)
        if (!combined && !authorEl) return;

        // Timestamp: prefer the ISO 8601 `datetime` attribute on <time>.
        // Fall back to deriving from the Discord snowflake ID (first 42 bits = ms since Discord epoch).
        // Never use textContent — it's a human-readable locale string that sorts incorrectly.
        var ts = timeEl ? (timeEl.getAttribute('datetime') || '') : '';
        if (!ts && msgId) {
            try {
                var ms = Number(BigInt(msgId) >> BigInt(22)) + 1420070400000;
                ts = new Date(ms).toISOString();
            } catch(e) {}
        }

        results.push({
            author: authorEl ? authorEl.textContent.trim() : '',
            time: ts,
            message: combined,
            msgId: msgId,
            domIndex: idx
        });
        idx++;
    });
    return results;
})())"#;

const MSG_SCROLL_INFO_SCRIPT: &str = r#"JSON.stringify((function() {
    var scrollers = document.querySelectorAll('[class*="scroller"]');
    var best = null;
    var bestH = 0;
    for (var i = 0; i < scrollers.length; i++) {
        var el = scrollers[i];
        if (el.scrollHeight > el.clientHeight && el.scrollHeight > bestH) {
            if (el.querySelector('[class*="message"]')) {
                best = el;
                bestH = el.scrollHeight;
            }
        }
    }
    if (!best) return { scroll_height: 0, client_height: 0, scroll_top: 0 };
    return { scroll_height: best.scrollHeight, client_height: best.clientHeight, scroll_top: best.scrollTop };
})())"#;

const SCROLL_TO_TOP_SCRIPT: &str = r#"(function() {
    var scrollers = document.querySelectorAll('[class*="scroller"]');
    for (var i = 0; i < scrollers.length; i++) {
        var el = scrollers[i];
        if (el.scrollHeight > el.clientHeight && el.querySelector('[class*="message"]')) {
            el.scrollTop = 0;
            return;
        }
    }
})()"#;

const SCROLL_TO_BOTTOM_SCRIPT: &str = r#"(function() {
    var scrollers = document.querySelectorAll('[class*="scroller"]');
    for (var i = 0; i < scrollers.length; i++) {
        var el = scrollers[i];
        if (el.scrollHeight > el.clientHeight && el.querySelector('[class*="message"]')) {
            el.scrollTop = el.scrollHeight;
            return;
        }
    }
})()"#;

fn scroll_to_script(pos: u64) -> String {
    format!(
        r#"(function() {{
            var scrollers = document.querySelectorAll('[class*="scroller"]');
            for (var i = 0; i < scrollers.length; i++) {{
                var el = scrollers[i];
                if (el.querySelector('[class*="message"]')) {{
                    el.scrollTop = {};
                    return;
                }}
            }}
        }})()"#,
        pos
    )
}

/// Open the Discord channel URL in the browser and verify the page loaded correctly.
///
/// Uses `agent-browser open <url>` to navigate directly, then checks `window.location.href`
/// to confirm the browser landed on the expected guild/channel IDs.
/// Returns `Err(AppError::WrongLocation(...))` if the URL didn't load as expected.
pub async fn open_channel(
    client: &AgentBrowserClient,
    channel_url: &str,
    guild_id: &str,
    channel_id: &str,
) -> AppResult<()> {
    info!("Opening channel URL: {}", channel_url);
    client.open(channel_url).await?;
    // Wait for Discord to finish navigating
    client.wait_ms(2000).await?;

    let href: String = client
        .eval_json("JSON.stringify(window.location.href)")
        .await?;

    let ok = href.contains(guild_id) && href.contains(channel_id);
    if ok {
        info!("Channel opened successfully: {}", href);
        Ok(())
    } else {
        let msg = format!(
            "Browser navigated to '{}' but expected guild={} channel={}. \
             Check that the channel_url in config is correct.",
            href, guild_id, channel_id
        );
        Err(AppError::WrongLocation(msg))
    }
}

#[allow(dead_code)]
/// Navigate to the given server and channel in Discord Web UI.
pub async fn navigate_to_channel(
    client: &AgentBrowserClient,
    server: &str,
    channel: &str,
) -> AppResult<()> {
    if !server.is_empty() {
        let target = serde_json::to_string(server).unwrap_or_else(|_| "\"\"".to_string());
        let script = format!(
            r#"JSON.stringify((function(target) {{
                var target_lower = target.toLowerCase();
                var items = document.querySelectorAll('[data-list-item-id*="guildsnav___"]');
                for (var i = 0; i < items.length; i++) {{
                    var el = items[i];
                    var listId = el.getAttribute('data-list-item-id') || '';
                    if (!/guildsnav___\d{{10,}}/.test(listId)) continue;
                    var name = (el.textContent || '').trim();
                    name = name.replace(/^[\d][\d,.]*\s*[^\uff0c,]*[\uff0c,]\s*/, '');
                    name = name.trim();
                    if (name.toLowerCase().indexOf(target_lower) === -1) continue;
                    var cls = el.getAttribute('class') || '';
                    if (cls.indexOf('selected') !== -1) {{
                        return {{ status: 'already_on' }};
                    }}
                    el.click();
                    return {{ status: 'switched' }};
                }}
                return {{ status: 'not_found' }};
            }})({}))"#,
            target
        );

        #[derive(Deserialize)]
        struct SwitchResult {
            status: String,
        }

        let res: SwitchResult = client.eval_json(&script).await?;
        match res.status.as_str() {
            "not_found" => {
                return Err(AppError::InvalidParams(format!(
                    "server not found: {}",
                    server
                )));
            }
            "switched" => {
                client.wait_ms(1000).await?;
            }
            _ => {} // already_on
        }
    }

    if !channel.is_empty() {
        let target = serde_json::to_string(channel).unwrap_or_else(|_| "\"\"".to_string());
        let try_click = format!(
            r#"JSON.stringify((function(target) {{
                var target_lower = target.toLowerCase();
                var els = document.querySelectorAll('[data-list-item-id^=channels___]');
                for (var i = 0; i < els.length; i++) {{
                    var el = els[i];
                    var label = (el.getAttribute('aria-label') || el.textContent || '').trim();
                    var commaIdx = label.search(/[,\uFF0C]/);
                    var name = commaIdx !== -1 ? label.substring(0, commaIdx).trim() : label;
                    if (name.toLowerCase().indexOf(target_lower) === -1) continue;
                    el.click();
                    return {{ status: 'switched' }};
                }}
                return {{ status: 'not_found' }};
            }})({}))"#,
            target
        );

        #[derive(Deserialize)]
        struct SwitchResult {
            status: String,
        }

        let res: SwitchResult = client.eval_json(&try_click).await?;
        if res.status != "switched" {
            // Scroll through channel list to find the channel
            let ch_scroll_info: &str = r#"JSON.stringify((function() {
                var all = document.querySelectorAll('[class*=scroller]');
                var best = null;
                var bestH = 0;
                for (var i = 0; i < all.length; i++) {
                    var el = all[i];
                    if (el.scrollHeight > el.clientHeight && el.scrollHeight > bestH) {
                        if (el.querySelector('[data-list-item-id^=channels___]')) {
                            best = el;
                            bestH = el.scrollHeight;
                        }
                    }
                }
                if (!best) { return { scroll_height: 0, client_height: 0, scroll_top: 0 }; }
                best.scrollTop = 0;
                return { scroll_height: best.scrollHeight, client_height: best.clientHeight, scroll_top: 0 };
            })())"#;

            let info: ScrollInfo = client.eval_json(ch_scroll_info).await?;
            client.wait_ms(300).await?;

            let mut found = false;
            if info.scroll_height > info.client_height {
                let step = info.client_height.max(200.0);
                let mut pos = step;
                while pos < info.scroll_height + step {
                    let scroll_js = format!(
                        "(function(){{ \
                            var all = document.querySelectorAll('[class*=scroller]'); \
                            for (var i=0; i<all.length; i++) {{ \
                                if (all[i].querySelector('[data-list-item-id^=channels___]')) {{ \
                                    all[i].scrollTop = {}; break; \
                                }} \
                            }} \
                        }})()",
                        pos as u64
                    );
                    client.eval(&scroll_js).await?;
                    client.wait_ms(300).await?;

                    let res: SwitchResult = client.eval_json(&try_click).await?;
                    if res.status == "switched" {
                        found = true;
                        break;
                    }
                    pos += step;
                }
            }

            if !found {
                return Err(AppError::InvalidParams(format!(
                    "channel not found: {}",
                    channel
                )));
            }
        }

        client.wait_ms(1000).await?;
    }

    Ok(())
}

/// Collect all currently visible messages in the chat area.
pub async fn collect_visible_messages(
    client: &AgentBrowserClient,
) -> AppResult<Vec<ScrapedMessage>> {
    let batch: Vec<serde_json::Map<String, Value>> = client.eval_json(COLLECT_SCRIPT).await?;
    let mut messages = Vec::new();

    for item in batch {
        let author = item
            .get("author")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let time = item
            .get("time")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let msg = item
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let msg_id = item
            .get("msgId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if msg.is_empty() && author.is_empty() {
            continue;
        }
        messages.push(ScrapedMessage::new(author, time, msg, msg_id));
    }

    Ok(messages)
}

/// Wait for Discord to finish loading history (scroll_height stabilizes).
async fn wait_for_scroll_stable(client: &AgentBrowserClient) -> AppResult<ScrollInfo> {
    let mut prev_height = 0.0f64;
    let mut stable_count = 0u32;

    loop {
        client.wait_ms(500).await?;
        let info: ScrollInfo = client.eval_json(MSG_SCROLL_INFO_SCRIPT).await?;

        if (info.scroll_height - prev_height).abs() < 1.0 {
            stable_count += 1;
            if stable_count >= 3 {
                return Ok(info);
            }
        } else {
            stable_count = 0;
        }
        prev_height = info.scroll_height;
    }
}

/// Scrape full message history.
///
/// Strategy:
/// 1. Phase 1: Scroll to top repeatedly to trigger Discord to load all history.
///    Each scroll-to-top increases scroll_height as older messages load.
///    Stop when scroll_height stabilizes (channel beginning reached).
/// 2. Phase 2: Sweep page-by-page from top to bottom, collecting all messages.
///    No page limit — runs until it naturally reaches the bottom.
///
/// Discord uses virtual scrolling (only renders messages near the viewport),
/// so we MUST sweep through every position to collect all messages.
pub async fn scrape_history(
    client: &AgentBrowserClient,
) -> AppResult<Vec<ScrapedMessage>> {
    // Phase 1: Scroll to the absolute top until scroll_height stabilizes.
    // No attempt limit — keep going until Discord has loaded all history.
    info!("Phase 1: Loading all history by scrolling to top...");
    let mut prev_scroll_height = 0.0f64;
    let mut top_attempts = 0u64;

    loop {
        client.eval(SCROLL_TO_TOP_SCRIPT).await?;
        let info = wait_for_scroll_stable(client).await?;

        top_attempts += 1;
        debug!(
            "Top attempt {}: scroll_height={:.0}, prev={:.0}",
            top_attempts, info.scroll_height, prev_scroll_height
        );

        if (info.scroll_height - prev_scroll_height).abs() < 1.0 {
            info!(
                "Reached channel beginning after {} attempts (scroll_height={:.0})",
                top_attempts, info.scroll_height
            );
            break;
        }

        prev_scroll_height = info.scroll_height;
    }

    // Phase 2: Sweep from top to bottom collecting all messages.
    info!("Phase 2: Sweeping from top to bottom...");
    client.eval(SCROLL_TO_TOP_SCRIPT).await?;
    client.wait_ms(1000).await?;

    let messages = sweep_to_bottom(client).await?;
    info!("History scrape complete: {} messages", messages.len());
    Ok(messages)
}

/// Sweep downward from the current scroll position to the bottom, collecting all messages.
/// No page limit — runs until it naturally reaches the bottom of the scroll area.
async fn sweep_to_bottom(
    client: &AgentBrowserClient,
) -> AppResult<Vec<ScrapedMessage>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut ordered: Vec<ScrapedMessage> = Vec::new();
    let mut page = 0u64;

    loop {
        let batch = collect_visible_messages(client).await?;
        let mut new_in_batch = 0;
        for msg in batch {
            if seen.insert(msg.dedup_hash.clone()) {
                ordered.push(msg);
                new_in_batch += 1;
            }
        }
        debug!(
            "Sweep page {}: {} new messages (total {})",
            page, new_in_batch, ordered.len()
        );
        page += 1;

        let info: ScrollInfo = client.eval_json(MSG_SCROLL_INFO_SCRIPT).await?;
        let step = info.client_height.max(300.0);
        let new_top = info.scroll_top + step;

        if info.scroll_top + info.client_height >= info.scroll_height - 1.0 {
            // At the bottom — one final collect to catch the last messages
            client.eval(SCROLL_TO_BOTTOM_SCRIPT).await?;
            client.wait_ms(500).await?;
            let batch = collect_visible_messages(client).await?;
            for msg in batch {
                if seen.insert(msg.dedup_hash.clone()) {
                    ordered.push(msg);
                }
            }
            break;
        }

        client.eval(&scroll_to_script(new_top as u64)).await?;
        client.wait_ms(500).await?;
    }

    Ok(ordered)
}

/// Catch up from the current Discord scroll position to the bottom.
///
/// Used on restart when the DB already has historical data. Discord navigates to the
/// first-unread message (or bottom if fully caught up), so sweeping from there to the
/// bottom is enough to pick up any messages missed while the service was offline.
pub async fn catch_up_to_bottom(
    client: &AgentBrowserClient,
) -> AppResult<Vec<ScrapedMessage>> {
    // Brief wait for Discord to finish rendering after navigation
    client.wait_ms(1000).await?;
    sweep_to_bottom(client).await
}

/// Poll for new messages without any scrolling.
///
/// Discord auto-displays new messages in the current view, so we just:
/// 1. Read the last visible message ID from the DOM (no scroll).
/// 2. If it matches `latest_discord_id` → nothing new, return empty.
/// 3. If it differs → collect all currently visible messages and return them.
///
/// This avoids the disruptive multi-page scroll-back that ran on every poll cycle.
pub async fn poll_new_messages(
    client: &AgentBrowserClient,
    latest_discord_id: Option<&str>,
) -> AppResult<Vec<ScrapedMessage>> {
    const LAST_ID_SCRIPT: &str = r#"JSON.stringify((function() {
        var els = document.querySelectorAll('[id^="message-content-"]');
        if (!els.length) return '';
        return els[els.length - 1].id.replace('message-content-', '');
    })())"#;

    let last_id: String = client.eval_json(LAST_ID_SCRIPT).await?;

    // If we have a known ID and it matches the last DOM message → nothing new
    if let Some(known) = latest_discord_id {
        if !last_id.is_empty() && last_id == known {
            return Ok(vec![]);
        }
    }

    // New messages are visible — collect without scrolling
    collect_visible_messages(client).await
}
