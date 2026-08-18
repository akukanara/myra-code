//! MyraCtx documentation lookup history cells.

use super::*;
use codex_app_server_protocol::MyraCtxItem;
use codex_app_server_protocol::MyraCtxStatus;

#[derive(Debug)]
pub(crate) struct MyraCtxCell {
    item: MyraCtxItem,
    start_time: Instant,
    animations_enabled: bool,
}

impl MyraCtxCell {
    pub(crate) fn new(item: MyraCtxItem, animations_enabled: bool) -> Self {
        Self {
            item,
            start_time: Instant::now(),
            animations_enabled,
        }
    }

    pub(crate) fn call_id(&self) -> &str {
        &self.item.id
    }

    pub(crate) fn update(&mut self, item: MyraCtxItem) {
        self.item = item;
    }

    fn is_complete(&self) -> bool {
        !matches!(self.item.status, MyraCtxStatus::InProgress)
    }

    fn title(&self) -> &'static str {
        match self.item.status {
            MyraCtxStatus::InProgress => "Checking current documentation",
            MyraCtxStatus::Completed => "Documentation consulted",
            MyraCtxStatus::Failed => "Documentation lookup unavailable",
        }
    }
}

impl HistoryCell for MyraCtxCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let bullet = match self.item.status {
            MyraCtxStatus::InProgress => activity_indicator(
                Some(self.start_time),
                MotionMode::from_animations_enabled(self.animations_enabled),
                ReducedMotionIndicator::StaticBullet,
            )
            .unwrap_or_else(|| "•".cyan()),
            MyraCtxStatus::Completed => "•".cyan().bold(),
            MyraCtxStatus::Failed => "•".red().bold(),
        };
        let header = Line::from(vec![
            bullet,
            " ".into(),
            "MYRACTX".cyan().bold(),
            "  ".into(),
            self.title().into(),
        ]);
        let detail = Line::from(vec![
            self.item.library.clone().bold(),
            "  ·  ".dim(),
            self.item.query.clone().dim(),
        ]);
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
        vec![Line::from(format!(
            "MyraCtx: {} · {} ({})",
            self.item.library,
            self.item.query,
            self.title()
        ))]
    }

    fn transcript_animation_tick(&self) -> Option<u64> {
        (!self.is_complete() && self.animations_enabled)
            .then(|| (self.start_time.elapsed().as_millis() / 50) as u64)
    }
}

pub(crate) fn new_myractx_call(item: MyraCtxItem, animations_enabled: bool) -> MyraCtxCell {
    MyraCtxCell::new(item, animations_enabled)
}
