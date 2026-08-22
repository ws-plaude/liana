//! The overlay half of [`ContextMenu`](super::ContextMenu), vendored from iced_aw 0.13.

use iced::{
    advanced::{
        layout::{Limits, Node},
        mouse::{self, Cursor},
        overlay, renderer,
        widget::{Operation, Tree},
        Clipboard, Layout, Shell,
    },
    keyboard, touch, window, Element, Event, Point, Size,
};

use super::State;

/// Draws the menu content positioned at the click, clamped to the viewport.
#[allow(missing_debug_implementations)]
pub struct ContextMenuOverlay<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    Renderer: 'a + renderer::Renderer,
{
    position: Point,
    tree: &'a mut Tree,
    content: Element<'a, Message, Theme, Renderer>,
    state: &'a mut State,
}

impl<'a, Message, Theme, Renderer> ContextMenuOverlay<'a, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: renderer::Renderer,
    Theme: 'a,
{
    pub(super) fn new<C>(
        position: Point,
        tree: &'a mut Tree,
        content: C,
        state: &'a mut State,
    ) -> Self
    where
        C: Into<Element<'a, Message, Theme, Renderer>>,
    {
        ContextMenuOverlay {
            position,
            tree,
            content: content.into(),
            state,
        }
    }

    pub(super) fn overlay(self) -> overlay::Element<'a, Message, Theme, Renderer> {
        overlay::Element::new(Box::new(self))
    }
}

impl<'a, Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for ContextMenuOverlay<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    Renderer: 'a + renderer::Renderer,
    Theme: 'a,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> Node {
        let limits = Limits::new(Size::ZERO, bounds);
        let max_size = limits.max();

        let mut content = self
            .content
            .as_widget_mut()
            .layout(self.tree, renderer, &limits);

        // Flip the menu back over the cursor rather than let it overflow.
        let mut position = self.position;
        if position.x + content.size().width > bounds.width {
            position.x = f32::max(0.0, position.x - content.size().width);
        }
        if position.y + content.size().height > bounds.height {
            position.y = f32::max(0.0, position.y - content.size().height);
        }

        content.move_to_mut(position);

        Node::with_children(max_size, vec![content])
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: Cursor,
    ) {
        let bounds = layout.bounds();
        let content_layout = layout
            .children()
            .next()
            .expect("context menu: layout should have a content layout.");

        self.content.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            content_layout,
            cursor,
            &bounds,
        );
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let layout_children = layout
            .children()
            .next()
            .expect("context menu: layout should have a content layout.");

        let mut forward_event_to_children = true;
        let mut capture_event = false;

        match &event {
            Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => {
                if *key == keyboard::Key::Named(keyboard::key::Named::Escape) {
                    self.state.show = false;
                    forward_event_to_children = false;
                    shell.capture_event();
                }
            }

            Event::Mouse(mouse::Event::ButtonPressed(
                mouse::Button::Left | mouse::Button::Right,
            ))
            | Event::Touch(touch::Event::FingerPressed { .. }) => {
                if cursor.is_over(layout_children.bounds()) {
                    capture_event = true;
                } else {
                    self.state.show = false;
                    forward_event_to_children = false;
                    shell.request_redraw();
                }
            }

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                // Close on release, because buttons send their message on release.
                self.state.show = false;
                capture_event = true;
            }

            Event::Window(window::Event::Resized { .. }) => {
                self.state.show = false;
                forward_event_to_children = false;
                capture_event = true;
            }

            _ => {}
        }

        if forward_event_to_children {
            self.content.as_widget_mut().update(
                self.tree,
                event,
                layout_children,
                cursor,
                renderer,
                clipboard,
                shell,
                &layout.bounds(),
            );
        }
        if capture_event {
            shell.capture_event();
        }
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        let content_layout = layout
            .children()
            .next()
            .expect("context menu: layout should have a content layout.");

        self.content
            .as_widget_mut()
            .operate(self.tree, content_layout, renderer, operation);
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();

        self.content.as_widget().mouse_interaction(
            self.tree,
            layout
                .children()
                .next()
                .expect("context menu: layout should have a content layout."),
            cursor,
            &bounds,
            renderer,
        )
    }
}
