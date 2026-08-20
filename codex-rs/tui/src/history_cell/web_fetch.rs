//! Gateway page-fetch history cells.

use super::*;
use codex_app_server_protocol::WebFetchItem;
use codex_app_server_protocol::WebFetchStatus;

/// Drops the parts of a URL that carry no information for a reader scanning a
/// transcript. The scheme is always http(s) and `www.` is never the
/// distinguishing part of a host.
fn display_url(url: &str) -> String {
    let trimmed = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let trimmed = trimmed.strip_prefix("www.").unwrap_or(trimmed);
    trimmed.trim_end_matches('/').to_string()
}

#[derive(Debug)]
pub(crate) struct WebFetchCell {
    item: WebFetchItem,
    start_time: Instant,
    animations_enabled: bool,
}

impl WebFetchCell {
    pub(crate) fn new(item: WebFetchItem, animations_enabled: bool) -> Self {
        Self {
            item,
            start_time: Instant::now(),
            animations_enabled,
        }
    }

    pub(crate) fn call_id(&self) -> &str {
        &self.item.id
    }

    pub(crate) fn update(&mut self, item: WebFetchItem) {
        self.item = item;
    }

    fn is_complete(&self) -> bool {
        !matches!(self.item.status, WebFetchStatus::InProgress)
    }

    fn title(&self) -> &'static str {
        match self.item.status {
            WebFetchStatus::InProgress => "Reading page",
            WebFetchStatus::Completed => "Page read",
            WebFetchStatus::Failed => "Page unavailable",
        }
    }
}

impl HistoryCell for WebFetchCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let bullet = match self.item.status {
            WebFetchStatus::InProgress => activity_indicator(
                Some(self.start_time),
                MotionMode::from_animations_enabled(self.animations_enabled),
                ReducedMotionIndicator::StaticBullet,
            )
            .unwrap_or_else(|| "•".magenta()),
            WebFetchStatus::Completed => "•".magenta().bold(),
            WebFetchStatus::Failed => "•".red().bold(),
        };
        let header = Line::from(vec![
            bullet,
            " ".into(),
            "WEB".magenta().bold(),
            "  ".into(),
            self.title().into(),
        ]);
        let url = display_url(&self.item.url);
        // The page title is what a reader recognizes; the URL is what they use
        // to go look. Show the title when the fetch returned one and keep the
        // URL alongside it rather than replacing it.
        let detail = match self.item.title.as_deref() {
            Some(title) => Line::from(vec![title.to_string().bold(), "  ·  ".dim(), url.dim()]),
            None => Line::from(url),
        };
        let detail_width = (width as usize).saturating_sub(4).max(1);
        let wrapped = adaptive_wrap_line(
            &detail,
            RtOptions::new(detail_width)
                .initial_indent("".into())
                .subsequent_indent("    ".into()),
        );
        let detail_lines = wrapped.iter().map(line_to_static).collect::<Vec<_>>();
        let mut lines = vec![line_to_static(&header)];
        lines.extend(prefix_lines(detail_lines, "  └ ".dim(), "    ".into()));
        lines
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let url = display_url(&self.item.url);
        let line = match self.item.title.as_deref() {
            Some(title) => format!("Web fetch: {title} · {url} ({})", self.title()),
            None => format!("Web fetch: {url} ({})", self.title()),
        };
        vec![Line::from(line)]
    }

    fn transcript_animation_tick(&self) -> Option<u64> {
        (!self.is_complete() && self.animations_enabled)
            .then(|| (self.start_time.elapsed().as_millis() / 50) as u64)
    }
}

pub(crate) fn new_web_fetch_call(item: WebFetchItem, animations_enabled: bool) -> WebFetchCell {
    WebFetchCell::new(item, animations_enabled)
}
