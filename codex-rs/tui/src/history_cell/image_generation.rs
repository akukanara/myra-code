//! Image-generation history cells.

use super::*;

/// Status strings carried by `ImageGenerationItem`. They arrive as free-form
/// strings from the extension, so anything unrecognized is treated as still
/// running rather than silently rendered as done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageGenerationState {
    InProgress,
    Completed,
    Failed,
}

impl ImageGenerationState {
    fn from_status(status: &str) -> Self {
        match status {
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::InProgress,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::InProgress => "Generating image",
            Self::Completed => "Image generated",
            Self::Failed => "Image generation failed",
        }
    }
}

#[derive(Debug)]
pub(crate) struct ImageGenerationCell {
    call_id: String,
    state: ImageGenerationState,
    prompt: Option<String>,
    saved_path: Option<AbsolutePathBuf>,
    start_time: Instant,
    animations_enabled: bool,
}

impl ImageGenerationCell {
    pub(crate) fn new(
        call_id: String,
        status: &str,
        prompt: Option<String>,
        saved_path: Option<AbsolutePathBuf>,
        animations_enabled: bool,
    ) -> Self {
        Self {
            call_id,
            state: ImageGenerationState::from_status(status),
            prompt,
            saved_path,
            start_time: Instant::now(),
            animations_enabled,
        }
    }

    pub(crate) fn call_id(&self) -> &str {
        &self.call_id
    }

    pub(crate) fn update(
        &mut self,
        status: &str,
        prompt: Option<String>,
        saved_path: Option<AbsolutePathBuf>,
    ) {
        self.state = ImageGenerationState::from_status(status);
        // The completion carries the backend's revised prompt; keep the
        // requested one when it does not.
        if prompt.is_some() {
            self.prompt = prompt;
        }
        if saved_path.is_some() {
            self.saved_path = saved_path;
        }
    }

    fn is_complete(&self) -> bool {
        !matches!(self.state, ImageGenerationState::InProgress)
    }

    fn saved_path_display(&self) -> Option<String> {
        let saved_path = self.saved_path.as_ref()?;
        // A file:// URL is clickable in most terminals; the plain path is the
        // fallback for a path that cannot be expressed as one.
        Some(
            Url::from_file_path(saved_path.as_path())
                .map(|url| url.to_string())
                .unwrap_or_else(|_| saved_path.display().to_string()),
        )
    }
}

impl HistoryCell for ImageGenerationCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let bullet = match self.state {
            ImageGenerationState::InProgress => activity_indicator(
                Some(self.start_time),
                MotionMode::from_animations_enabled(self.animations_enabled),
                ReducedMotionIndicator::StaticBullet,
            )
            .unwrap_or_else(|| "•".yellow()),
            ImageGenerationState::Completed => "•".yellow().bold(),
            ImageGenerationState::Failed => "•".red().bold(),
        };
        let header = Line::from(vec![
            bullet,
            " ".into(),
            "IMAGE".yellow().bold(),
            "  ".into(),
            self.state.title().into(),
        ]);

        let detail_width = (width as usize).saturating_sub(4).max(1);
        let mut detail_lines: Vec<Line<'static>> = Vec::new();
        if let Some(prompt) = self.prompt.as_deref().filter(|prompt| !prompt.is_empty()) {
            let prompt_line = Line::from(prompt.to_string());
            let wrapped = adaptive_wrap_line(
                &prompt_line,
                RtOptions::new(detail_width)
                    .initial_indent("".into())
                    .subsequent_indent("    ".into()),
            );
            detail_lines.extend(wrapped.iter().map(line_to_static));
        }
        if let Some(saved_path) = self.saved_path_display() {
            detail_lines.push(Line::from(vec!["Saved to ".dim(), saved_path.into()]));
        }
        if detail_lines.is_empty() {
            return vec![line_to_static(&header)];
        }

        let mut lines = vec![line_to_static(&header)];
        lines.extend(prefix_lines(detail_lines, "  └ ".dim(), "    ".into()));
        lines
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(format!(
            "Image generation: {}",
            self.state.title()
        ))];
        if let Some(prompt) = self.prompt.as_deref().filter(|prompt| !prompt.is_empty()) {
            lines.push(Line::from(prompt.to_string()));
        }
        if let Some(saved_path) = self.saved_path_display() {
            lines.push(Line::from(format!("Saved to {saved_path}")));
        }
        lines
    }

    fn transcript_animation_tick(&self) -> Option<u64> {
        (!self.is_complete() && self.animations_enabled)
            .then(|| (self.start_time.elapsed().as_millis() / 50) as u64)
    }
}

pub(crate) fn new_image_generation_cell(
    call_id: String,
    status: &str,
    prompt: Option<String>,
    saved_path: Option<AbsolutePathBuf>,
    animations_enabled: bool,
) -> ImageGenerationCell {
    ImageGenerationCell::new(call_id, status, prompt, saved_path, animations_enabled)
}
