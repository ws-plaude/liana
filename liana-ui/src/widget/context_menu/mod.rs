//! A context menu shown on right click, vendored from iced_aw 0.13.
//!
//! Trimmed to what this crate uses: the upstream widget is generic over a
//! `Catalog` whose only effect is the backdrop colour, which we always set to
//! transparent, so the style plumbing and the backdrop quad are both gone.

mod overlay;

use iced::{
    advanced::{
        layout::{Limits, Node},
        mouse::{self, Button, Cursor},
        overlay as iced_overlay, renderer,
        widget::{tree, Operation, Tree},
        Clipboard, Layout, Shell, Widget,
    },
    Element, Event, Length, Point, Rectangle, Size, Vector,
};

use overlay::ContextMenuOverlay;

/// A widget showing `overlay` on top of `underlay` when right clicked.
#[allow(missing_debug_implementations)]
pub struct ContextMenu<'a, Overlay, Message, Theme, Renderer>
where
    Overlay: Fn() -> Element<'a, Message, Theme, Renderer>,
    Message: Clone,
    Renderer: renderer::Renderer,
{
    underlay: Element<'a, Message, Theme, Renderer>,
    overlay: Overlay,
}

impl<'a, Overlay, Message, Theme, Renderer> ContextMenu<'a, Overlay, Message, Theme, Renderer>
where
    Overlay: Fn() -> Element<'a, Message, Theme, Renderer>,
    Message: Clone,
    Renderer: renderer::Renderer,
{
    /// `overlay` is built lazily, each time the menu is shown.
    pub fn new<U>(underlay: U, overlay: Overlay) -> Self
    where
        U: Into<Element<'a, Message, Theme, Renderer>>,
    {
        ContextMenu {
            underlay: underlay.into(),
            overlay,
        }
    }
}

impl<'a, Content, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ContextMenu<'a, Content, Message, Theme, Renderer>
where
    Content: 'a + Fn() -> Element<'a, Message, Theme, Renderer>,
    Message: 'a + Clone,
    Renderer: 'a + renderer::Renderer,
{
    fn size(&self) -> Size<Length> {
        self.underlay.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &Limits) -> Node {
        self.underlay
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        state: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: Cursor,
        viewport: &Rectangle,
    ) {
        self.underlay.as_widget().draw(
            &state.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::new())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.underlay), Tree::new((self.overlay)())]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.underlay, &(self.overlay)()]);
    }

    fn operate<'b>(
        &'b mut self,
        state: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation<()>,
    ) {
        let s: &mut State = state.state.downcast_mut();

        if s.show {
            let mut content = (self.overlay)();
            content.as_widget_mut().diff(&mut state.children[1]);

            content
                .as_widget_mut()
                .operate(&mut state.children[1], layout, renderer, operation);
        } else {
            self.underlay.as_widget_mut().operate(
                &mut state.children[0],
                layout,
                renderer,
                operation,
            );
        }
    }

    fn update(
        &mut self,
        state: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if *event == Event::Mouse(mouse::Event::ButtonPressed(Button::Right)) {
            let bounds = layout.bounds();

            if cursor.is_over(bounds) {
                let s: &mut State = state.state.downcast_mut();
                s.cursor_position = cursor.position().unwrap_or_default();
                s.show = !s.show;
                shell.capture_event();
            }
        }

        self.underlay.as_widget_mut().update(
            &mut state.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        state: &Tree,
        layout: Layout<'_>,
        cursor: Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.underlay.as_widget().mouse_interaction(
            &state.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<iced_overlay::Element<'b, Message, Theme, Renderer>> {
        let s: &mut State = tree.state.downcast_mut();

        if !s.show {
            return self.underlay.as_widget_mut().overlay(
                &mut tree.children[0],
                layout,
                renderer,
                viewport,
                translation,
            );
        }

        let position = s.cursor_position;
        let mut content = (self.overlay)();
        content.as_widget_mut().diff(&mut tree.children[1]);
        Some(
            ContextMenuOverlay::new(position + translation, &mut tree.children[1], content, s)
                .overlay(),
        )
    }
}

impl<'a, Content, Message, Theme, Renderer> From<ContextMenu<'a, Content, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Content: 'a + Fn() -> Self,
    Message: 'a + Clone,
    Renderer: 'a + renderer::Renderer,
    Theme: 'a,
{
    fn from(menu: ContextMenu<'a, Content, Message, Theme, Renderer>) -> Self {
        Element::new(menu)
    }
}

/// Shared between the widget and its overlay.
#[derive(Debug, Default)]
struct State {
    show: bool,
    /// Where the right click happened, so the overlay opens under the cursor.
    cursor_position: Point,
}

impl State {
    const fn new() -> Self {
        Self {
            show: false,
            cursor_position: Point::ORIGIN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use crate::widget::Renderer;

    #[derive(Clone)]
    enum TestMessage {}

    type TestMenu<'a> = ContextMenu<
        'a,
        fn() -> Element<'a, TestMessage, Theme, Renderer>,
        TestMessage,
        Theme,
        Renderer,
    >;

    fn overlay_content() -> Element<'static, TestMessage, Theme, Renderer> {
        iced::widget::Text::new("overlay").into()
    }

    fn menu<'a>() -> TestMenu<'a> {
        ContextMenu::new(iced::widget::Text::new("underlay"), overlay_content)
    }

    #[test]
    fn starts_hidden_at_the_origin() {
        let state = State::new();
        assert!(!state.show);
        assert_eq!(state.cursor_position, Point::ORIGIN);
    }

    #[test]
    fn size_is_delegated_to_the_underlay() {
        let size = Widget::<TestMessage, Theme, Renderer>::size(&menu());
        assert_eq!(size.width, Length::Shrink);
        assert_eq!(size.height, Length::Shrink);
    }

    #[test]
    fn tracks_underlay_and_overlay_as_children() {
        let children = Widget::<TestMessage, Theme, Renderer>::children(&menu());
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn tag_is_the_menu_state() {
        let tag = Widget::<TestMessage, Theme, Renderer>::tag(&menu());
        assert_eq!(tag, tree::Tag::of::<State>());
    }
}
