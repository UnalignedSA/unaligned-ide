use editor::{Editor, EditorElement, EditorStyle, MultiBufferOffset, ToOffset};
use gpui::{Action, App, ClickEvent, Context, Entity, FocusHandle, IntoElement, TextStyle, Window};
use settings::Settings;
use theme_settings::ThemeSettings;
use ui::{IconButton, IconButtonShape, InputShell, InputShellGround, InputShellSize};
use ui::{Tooltip, prelude::*};

pub(super) enum HistoryNavigationDirection {
    Previous,
    Next,
}

pub(super) fn should_navigate_history(
    editor: &Entity<Editor>,
    direction: HistoryNavigationDirection,
    cx: &App,
) -> bool {
    let editor_ref = editor.read(cx);
    let snapshot = editor_ref.buffer().read(cx).snapshot(cx);
    if snapshot.max_point().row == 0 {
        return true;
    }
    let selections = editor_ref.selections.disjoint_anchors();
    if let [selection] = selections {
        let offset = selection.end.to_offset(&snapshot);
        match direction {
            HistoryNavigationDirection::Previous => offset == MultiBufferOffset(0),
            HistoryNavigationDirection::Next => offset == snapshot.len(),
        }
    } else {
        true
    }
}

pub(super) enum ActionButtonState {
    Disabled,
    Toggled,
}

/// Replace and Replace All: the same icon button as the others of the bar,
/// outlined, since it is the one that changes the text.
pub(super) fn render_replace_action_button<T: 'static, A: Action>(
    id_prefix: &'static str,
    icon: ui::IconName,
    button_state: Option<ActionButtonState>,
    tooltip: &'static str,
    action: &'static A,
    focus_handle: FocusHandle,
    cx: &Context<T>,
    handler: fn(&mut T, &A, &mut Window, &mut Context<T>),
) -> IconButton {
    render_local_action_button(
        id_prefix,
        icon,
        button_state,
        tooltip,
        action,
        focus_handle,
        cx,
        handler,
    )
    .style(ButtonStyle::Outlined)
}

pub(super) fn render_local_action_button<T: 'static, A: Action>(
    id_prefix: &'static str,
    icon: ui::IconName,
    button_state: Option<ActionButtonState>,
    tooltip: &'static str,
    action: &'static A,
    focus_handle: FocusHandle,
    cx: &Context<T>,
    handler: fn(&mut T, &A, &mut Window, &mut Context<T>),
) -> IconButton {
    IconButton::new(
        SharedString::from(format!("{id_prefix}-{}", action.name())),
        icon,
    )
    .shape(IconButtonShape::Square)
    .icon_size(IconSize::Small)
    .aria_label(tooltip)
    .on_click(cx.listener({
        let focus_handle = focus_handle.clone();
        move |this, _: &ClickEvent, window, cx| {
            if !focus_handle.is_focused(window) {
                window.focus(&focus_handle, cx);
            }
            handler(this, action, window, cx);
        }
    }))
    .tooltip(move |_window, cx| Tooltip::for_action_in(tooltip, action, &focus_handle, cx))
    .when_some(button_state, |this, state| match state {
        ActionButtonState::Toggled => this.toggle_state(true),
        ActionButtonState::Disabled => this.disabled(true),
    })
}

/// The box of a search input: `ui::InputShell`, small, with its focus and
/// error lines. `ground` follows the surface the input sits on: sunken inside
/// the raised find widget, raised when it sits on the editor's own ground.
pub(crate) fn input_base_styles(
    focused: bool,
    has_error: bool,
    ground: InputShellGround,
    map: impl FnOnce(InputShell) -> InputShell,
) -> InputShell {
    InputShell::new()
        .size(InputShellSize::Small)
        .ground(ground)
        .focused(focused)
        .invalid(has_error)
        .min_w_32()
        .map(map)
}

/// The same box with a "Find in Results" caption before the text.
pub(crate) fn filter_search_results_input(
    focused: bool,
    has_error: bool,
    ground: InputShellGround,
    map: impl FnOnce(InputShell) -> InputShell,
) -> InputShell {
    input_base_styles(focused, has_error, ground, map).start_slot(
        Label::new("Find in Results")
            .size(LabelSize::Small)
            .color(Color::Muted)
            .mr_1(),
    )
}

pub(crate) fn render_text_input(
    editor: &Entity<Editor>,
    color_override: Option<Color>,
    app: &App,
) -> impl IntoElement {
    let (color, use_syntax) = if editor.read(app).read_only(app) {
        (app.theme().colors().text_disabled, false)
    } else {
        match color_override {
            Some(color_override) => (color_override.color(app), false),
            None => (app.theme().colors().text, true),
        }
    };

    let settings = ThemeSettings::get_global(app);
    let text_style = TextStyle {
        color,
        font_family: settings.buffer_font.family.clone(),
        font_features: settings.buffer_font.features.clone(),
        font_fallbacks: settings.buffer_font.fallbacks.clone(),
        font_size: rems(0.8125).into(),
        font_weight: settings.buffer_font.weight,
        line_height: relative(1.3),
        ..TextStyle::default()
    };

    // No ground of its own: the editor shows the input shell under it, in
    // every state of the shell (rest, hover).
    let mut editor_style = EditorStyle {
        local_player: app.theme().players().local(),
        text: text_style,
        ..EditorStyle::default()
    };
    if use_syntax {
        editor_style.syntax = app.theme().syntax().clone();
    }

    EditorElement::new(editor, editor_style)
}

/// This element makes all search inputs align as if they were in the same column
pub(crate) fn alignment_element() -> Div {
    div().size(rems_from_px(22_f32)).flex_none()
}

/// The match counter: numbers that change, so in the code font (D3), small.
pub(crate) fn match_count_label(text: String, color: Color, cx: &App) -> impl IntoElement {
    div()
        .font_buffer(cx)
        .text_size(rems_from_px(12_f32))
        .text_color(color.color(cx))
        .whitespace_nowrap()
        .child(text)
}

/// "3 of 12", or "No results" when there is nothing to step through.
pub(crate) fn match_count_text(active_match: Option<usize>, total: usize, more: bool) -> String {
    match active_match {
        Some(index) if total > 0 => {
            format!("{} of {}{}", index + 1, total, if more { "+" } else { "" })
        }
        _ => "No results".to_string(),
    }
}
