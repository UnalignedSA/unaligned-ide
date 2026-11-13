use std::time::Duration;

use gpui::{ElementId, Task};
use ui::prelude::*;

/// How often the indicator takes its next step.
const STEP: Duration = Duration::from_millis(400);

/// The only thing that moves in the chat while work is in progress: `·`,
/// `··`, `···` in the code font.
///
/// It is driven by one timer and repaints nothing between two steps, where
/// an animation repaints the whole window on every frame. The timer belongs
/// to a view that exists only while the element is on screen: when the work
/// ends the element is no longer rendered, the view is dropped and the timer
/// is cancelled with it.
///
/// `id` tells two indicators of the same parent apart.
#[derive(IntoElement)]
pub struct WorkingIndicator {
    id: ElementId,
}

impl WorkingIndicator {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self { id: id.into() }
    }
}

impl RenderOnce for WorkingIndicator {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        window.with_id(self.id, |window| {
            window.use_state(cx, |_, cx| WorkingIndicatorState::new(cx))
        })
    }
}

struct WorkingIndicatorState {
    step: usize,
    _ticker: Task<()>,
}

impl WorkingIndicatorState {
    fn new(cx: &mut Context<Self>) -> Self {
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(STEP).await;
                let alive = this.update(cx, |this, cx| {
                    this.step = (this.step + 1) % 3;
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        Self {
            step: 0,
            _ticker: ticker,
        }
    }
}

impl Render for WorkingIndicatorState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dots = match self.step {
            0 => "·",
            1 => "··",
            _ => "···",
        };
        // A fixed width: the text next to it must not move with the dots.
        div().w(rems_from_px(20_f32)).flex_none().child(
            Label::new(dots)
                .size(LabelSize::Small)
                .color(Color::Muted)
                .buffer_font(cx)
                .single_line(),
        )
    }
}
