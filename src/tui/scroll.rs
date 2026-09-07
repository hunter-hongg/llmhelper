/// Shared scroll behaviour for the static scrollable viewers (report,
/// request). Any viewer state that exposes a scroll offset, a viewport
/// height and a content length gets the whole navigation API for free, so
/// the two TUIs cannot drift apart.
pub trait Scrollable {
    fn scroll(&self) -> usize;
    fn viewport_height(&self) -> usize;
    fn content_length(&self) -> usize;
    fn set_scroll(&mut self, offset: usize);
    fn set_viewport(&mut self, height: usize);

    /// Largest valid scroll offset: the first line where the viewport
    /// bottom touches the document end. The viewport is at least one line
    /// so a document always remains scrollable to its last line.
    fn max_scroll(&self) -> usize {
        self.content_length()
            .saturating_sub(self.viewport_height().max(1))
    }

    /// Record the body area height and clamp the scroll offset into bounds.
    fn set_viewport_height(&mut self, height: usize) {
        self.set_viewport(height);
        self.set_scroll(self.scroll().min(self.max_scroll()));
    }

    fn scroll_down(&mut self) {
        self.set_scroll((self.scroll() + 1).min(self.max_scroll()));
    }

    fn scroll_up(&mut self) {
        self.set_scroll(self.scroll().saturating_sub(1));
    }

    /// Half a viewport per page so the reader keeps context around the jump.
    fn page_down(&mut self) {
        self.set_scroll((self.scroll() + self.page_size()).min(self.max_scroll()));
    }

    fn page_up(&mut self) {
        self.set_scroll(self.scroll().saturating_sub(self.page_size()));
    }

    fn page_size(&self) -> usize {
        (self.viewport_height() / 2).max(1)
    }

    fn scroll_top(&mut self) {
        self.set_scroll(0);
    }

    fn scroll_bottom(&mut self) {
        self.set_scroll(self.max_scroll());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct S {
        scroll: usize,
        viewport: usize,
        lines: usize,
    }

    impl Scrollable for S {
        fn scroll(&self) -> usize {
            self.scroll
        }
        fn viewport_height(&self) -> usize {
            self.viewport
        }
        fn content_length(&self) -> usize {
            self.lines
        }
        fn set_scroll(&mut self, offset: usize) {
            self.scroll = offset;
        }
        fn set_viewport(&mut self, height: usize) {
            self.viewport = height;
        }
    }

    fn s(lines: usize, viewport: usize) -> S {
        S {
            scroll: 0,
            viewport,
            lines,
        }
    }

    #[test]
    fn clamps_at_document_end() {
        let mut v = s(40, 10);
        for _ in 0..100 {
            v.scroll_down();
        }
        assert_eq!(v.scroll, 30);
    }

    #[test]
    fn empty_document_never_scrolls() {
        let mut v = s(0, 10);
        v.scroll_down();
        v.page_down();
        v.scroll_bottom();
        assert_eq!(v.scroll, 0);
    }

    #[test]
    fn zero_viewport_reaches_last_line() {
        let mut v = s(40, 0);
        v.scroll_bottom();
        assert_eq!(v.scroll, 39);
    }

    #[test]
    fn growing_viewport_clamps() {
        let mut v = s(40, 10);
        v.scroll_bottom();
        v.set_viewport_height(20);
        assert_eq!(v.scroll, 20);
    }

    #[test]
    fn zero_viewport_pages_by_one() {
        let mut v = s(40, 0);
        v.page_down();
        assert_eq!(v.scroll, 1);
    }
}
