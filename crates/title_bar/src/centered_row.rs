//! The title bar's row: a left group, a right group, and between them one
//! element whose middle is the middle of the whole bar.
//!
//! The window controls (traffic lights, caption buttons, the desktop's buttons
//! on Linux) take room at the ends of the bar, outside this row, and the two
//! groups do not have the same width. Flex cannot keep the middle element on
//! the bar's middle through both, so the row measures its groups and places
//! the three itself. The arithmetic is [`arrange`], a function of plain
//! numbers.

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Style, Window, point, px, relative, size,
};

/// What [`arrange`] needs to know, all in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RowMetrics {
    /// Width of the row itself.
    pub width: Pixels,
    /// Distance from the bar's left end to the row's left end.
    pub left_inset: Pixels,
    /// Distance from the row's right end to the bar's right end.
    pub right_inset: Pixels,
    /// Width the left group asks for.
    pub left_content: Pixels,
    /// Width the right group asks for.
    pub right_content: Pixels,
    /// Width of the middle element when there is room.
    pub center_preferred: Pixels,
    /// Width under which the middle element is not shrunk.
    pub center_min: Pixels,
    /// Space kept between the middle element and each group.
    pub gap: Pixels,
}

/// Where the three parts go, in the row's own coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RowArrangement {
    pub left_width: Pixels,
    pub center_x: Pixels,
    pub center_width: Pixels,
    pub right_x: Pixels,
    pub right_width: Pixels,
}

impl RowArrangement {
    /// Distance from the bar's left end to the middle of the middle element.
    #[cfg(test)]
    fn center_in_bar(&self, metrics: &RowMetrics) -> Pixels {
        metrics.left_inset + self.center_x + self.center_width / 2.
    }
}

/// Places the three parts of the row.
///
/// The groups keep the width they ask for. The middle element is centred on
/// the bar (the row plus both insets), at its preferred width, and it is what
/// shrinks when a group comes close: it stays centred down to its minimum
/// width. Under that it keeps the minimum and slides off the middle, and when
/// even that does not fit the left group is cut, then the middle element.
pub(crate) fn arrange(metrics: RowMetrics) -> RowArrangement {
    let zero = px(0.);
    let width = metrics.width.max(zero);
    let gap = metrics.gap.max(zero);
    let center_min = metrics.center_min.max(zero);
    let center_preferred = metrics.center_preferred.max(center_min);

    let right_width = metrics.right_content.max(zero).min(width);
    let right_x = width - right_width;
    let left_width = metrics
        .left_content
        .max(zero)
        .min((right_x - center_min - gap * 2.).max(zero));

    // The span the middle element may use.
    let end = (right_x - gap).max(zero);
    let start = (left_width + gap).min(end);
    let room = end - start;

    // The bar's middle, seen from the row.
    let middle = (metrics.left_inset + width + metrics.right_inset) / 2. - metrics.left_inset;
    let centred_room = (middle - start).min(end - middle) * 2.;

    let (center_x, center_width) = if centred_room >= center_min {
        // Whole pixels, so the outline stays sharp; the middle is at most
        // half a pixel off.
        let center_width = centred_room.min(center_preferred).floor();
        ((middle - center_width / 2.).round(), center_width)
    } else {
        let center_width = center_min.min(room).floor();
        let center_x = (middle - center_width / 2.)
            .round()
            .min(end - center_width)
            .max(start);
        (center_x, center_width)
    };

    RowArrangement {
        left_width,
        center_x,
        center_width,
        right_x,
        right_width,
    }
}

/// See the module's documentation. `center` should be `w_full`: it is given
/// exactly the width [`arrange`] decides.
pub(crate) struct CenteredRow {
    left: AnyElement,
    center: AnyElement,
    right: AnyElement,
    left_inset: Pixels,
    right_inset: Pixels,
    center_preferred: Pixels,
    center_min: Pixels,
    gap: Pixels,
}

impl CenteredRow {
    pub(crate) fn new(
        left: impl IntoElement,
        center: impl IntoElement,
        right: impl IntoElement,
    ) -> Self {
        Self {
            left: left.into_any_element(),
            center: center.into_any_element(),
            right: right.into_any_element(),
            left_inset: px(0.),
            right_inset: px(0.),
            center_preferred: px(0.),
            center_min: px(0.),
            gap: px(0.),
        }
    }

    /// What the bar takes at each end, outside this row.
    pub(crate) fn insets(mut self, left: Pixels, right: Pixels) -> Self {
        self.left_inset = left;
        self.right_inset = right;
        self
    }

    pub(crate) fn center_width(mut self, preferred: Pixels, min: Pixels) -> Self {
        self.center_preferred = preferred;
        self.center_min = min;
        self
    }

    pub(crate) fn gap(mut self, gap: Pixels) -> Self {
        self.gap = gap;
        self
    }
}

impl Element for CenteredRow {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // The row fills its parent; its children are laid out in `prepaint`,
        // once its width is known.
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let height = AvailableSpace::Definite(bounds.size.height);
        let unconstrained = size(AvailableSpace::MaxContent, height);
        let left_content = self.left.layout_as_root(unconstrained, window, cx).width;
        let right_content = self.right.layout_as_root(unconstrained, window, cx).width;

        let arrangement = arrange(RowMetrics {
            width: bounds.size.width,
            left_inset: self.left_inset,
            right_inset: self.right_inset,
            left_content,
            right_content,
            center_preferred: self.center_preferred,
            center_min: self.center_min,
            gap: self.gap,
        });

        let origin = bounds.origin;
        let exactly = |width: Pixels| size(AvailableSpace::Definite(width), height);
        self.left
            .prepaint_as_root(origin, exactly(arrangement.left_width), window, cx);
        self.center.prepaint_as_root(
            origin + point(arrangement.center_x, px(0.)),
            exactly(arrangement.center_width),
            window,
            cx,
        );
        self.right.prepaint_as_root(
            origin + point(arrangement.right_x, px(0.)),
            exactly(arrangement.right_width),
            window,
            cx,
        );
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.left.paint(window, cx);
        self.center.paint(window, cx);
        self.right.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFERRED: Pixels = px(440.);
    const MIN: Pixels = px(72.);
    const GAP: Pixels = px(8.);

    /// A bar `bar_width` wide whose ends are taken by `left_inset` and
    /// `right_inset`.
    fn bar(
        bar_width: f32,
        (left_inset, right_inset): (f32, f32),
        (left_content, right_content): (f32, f32),
    ) -> RowMetrics {
        RowMetrics {
            width: px(bar_width - left_inset - right_inset),
            left_inset: px(left_inset),
            right_inset: px(right_inset),
            left_content: px(left_content),
            right_content: px(right_content),
            center_preferred: PREFERRED,
            center_min: MIN,
            gap: GAP,
        }
    }

    fn assert_centred(metrics: RowMetrics, bar_width: f32) -> RowArrangement {
        let arrangement = arrange(metrics);
        let off = (arrangement.center_in_bar(&metrics) - px(bar_width / 2.)).abs();
        assert!(
            off <= px(0.5),
            "{off:?} off the middle: {arrangement:?} for {metrics:?}"
        );
        assert_no_overlap(&metrics, &arrangement);
        arrangement
    }

    fn assert_no_overlap(metrics: &RowMetrics, arrangement: &RowArrangement) {
        assert!(arrangement.left_width >= px(0.));
        assert!(arrangement.center_width >= px(0.));
        assert!(arrangement.center_x >= arrangement.left_width);
        assert!(arrangement.center_x + arrangement.center_width <= arrangement.right_x);
        assert_eq!(
            arrangement.right_x + arrangement.right_width,
            metrics.width.max(px(0.))
        );
    }

    // The insets below are those `platform_title_bar` reports: macOS 78 (or
    // 71) and 0; Windows 8 and 108; Linux 8 and 108 with three buttons on the
    // right, 108 and 0 with three on the left, 44 and 44 with one each side.
    const MACOS: (f32, f32) = (78., 0.);
    const WINDOWS: (f32, f32) = (8., 108.);
    const LINUX_RIGHT: (f32, f32) = (8., 108.);
    const LINUX_LEFT: (f32, f32) = (108., 0.);
    const LINUX_SPLIT: (f32, f32) = (44., 44.);
    const NO_CONTROLS: (f32, f32) = (8., 0.);

    #[test]
    fn test_centred_at_full_width_on_every_platform() {
        for insets in [
            MACOS,
            WINDOWS,
            LINUX_RIGHT,
            LINUX_LEFT,
            LINUX_SPLIT,
            NO_CONTROLS,
        ] {
            for bar_width in [1440., 1441., 1280., 1000.] {
                let arrangement = assert_centred(bar(bar_width, insets, (150., 120.)), bar_width);
                assert_eq!(arrangement.center_width, PREFERRED, "{insets:?}");
            }
        }
    }

    #[test]
    fn test_windows_caption_buttons_do_not_pull_the_centre_left() {
        // 1440px window, 8px padding left, 108px of caption buttons right:
        // the row is 1324 wide and its own middle is 50px left of the bar's.
        let metrics = bar(1440., WINDOWS, (150., 120.));
        let arrangement = arrange(metrics);
        assert_eq!(arrangement.center_width, px(440.));
        // 720 - 220 = 500 in the bar, so 492 in the row.
        assert_eq!(arrangement.center_x, px(492.));
        assert_eq!(arrangement.center_in_bar(&metrics), px(720.));
    }

    #[test]
    fn test_linux_left_hand_buttons_do_not_pull_the_centre_right() {
        let metrics = bar(1440., LINUX_LEFT, (150., 120.));
        let arrangement = arrange(metrics);
        // 500 in the bar, so 392 in the row.
        assert_eq!(arrangement.center_x, px(392.));
        assert_eq!(arrangement.center_in_bar(&metrics), px(720.));
    }

    #[test]
    fn test_a_wide_group_shrinks_the_centre_and_keeps_it_centred() {
        // The right group grows by an "Update Available" button.
        for insets in [MACOS, WINDOWS, LINUX_LEFT, LINUX_SPLIT] {
            let arrangement = assert_centred(bar(1000., insets, (150., 290.)), 1000.);
            assert!(arrangement.center_width < PREFERRED, "{insets:?}");
            assert!(arrangement.center_width >= MIN, "{insets:?}");
        }

        // macOS, 1000px: the right group ends 290 + 8 from the right end, so
        // the centre has 500 - 298 = 202 on each side of the middle.
        let arrangement = arrange(bar(1000., MACOS, (150., 290.)));
        assert_eq!(arrangement.center_width, px(404.));

        // A long project and branch name on the left does the same.
        let arrangement = assert_centred(bar(1280., WINDOWS, (460., 120.)), 1280.);
        // 640 - (8 + 460 + 8) = 164 on each side.
        assert_eq!(arrangement.center_width, px(328.));
    }

    #[test]
    fn test_under_the_minimum_the_centre_slides_instead_of_shrinking() {
        // 700px on Windows: the left group ends at 348 in the bar, two pixels
        // short of the middle, so nothing centred fits.
        let metrics = bar(700., WINDOWS, (340., 112.));
        let arrangement = arrange(metrics);
        assert_no_overlap(&metrics, &arrangement);
        assert_eq!(arrangement.center_width, MIN);
        assert_eq!(arrangement.left_width, px(340.));
        // As near the middle as the left group allows: right after its gap.
        assert_eq!(arrangement.center_x, px(348.));
    }

    #[test]
    fn test_too_narrow_cuts_the_left_group_then_the_centre() {
        // The left group gives way first, the centre keeps its minimum.
        let metrics = bar(400., WINDOWS, (300., 112.));
        let arrangement = arrange(metrics);
        assert_no_overlap(&metrics, &arrangement);
        assert_eq!(arrangement.right_width, px(112.));
        assert_eq!(arrangement.center_width, MIN);
        assert_eq!(arrangement.left_width, px(284. - 112. - 72. - 16.));

        // Nothing left for the left group: the centre takes what remains.
        let metrics = bar(260., WINDOWS, (300., 112.));
        let arrangement = arrange(metrics);
        assert_no_overlap(&metrics, &arrangement);
        assert_eq!(arrangement.left_width, px(0.));
        assert_eq!(arrangement.center_width, px(144. - 112. - 16.));

        // Not even the right group fits.
        let metrics = bar(200., WINDOWS, (300., 112.));
        let arrangement = arrange(metrics);
        assert_no_overlap(&metrics, &arrangement);
        assert_eq!(arrangement.right_width, px(84.));
        assert_eq!(arrangement.left_width, px(0.));
        assert_eq!(arrangement.center_width, px(0.));

        let metrics = bar(100., WINDOWS, (300., 112.));
        let arrangement = arrange(metrics);
        assert_eq!(arrangement.right_width, px(0.));
        assert_eq!(arrangement.center_width, px(0.));
    }

    #[test]
    fn test_empty_groups() {
        let arrangement = assert_centred(bar(1440., WINDOWS, (0., 0.)), 1440.);
        assert_eq!(arrangement.center_width, PREFERRED);
        assert_eq!(arrangement.left_width, px(0.));
        assert_eq!(arrangement.right_width, px(0.));
    }
}
