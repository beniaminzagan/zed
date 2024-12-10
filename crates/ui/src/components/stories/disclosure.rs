use gpui::{Model, Render};
use story::Story;

use crate::prelude::*;
use crate::Disclosure;

pub struct DisclosureStory;

impl Render for DisclosureStory {
    fn render(
        &mut self,
        model: &Model<Self>,
        _window: &mut gpui::Window,
        _cx: &mut AppContext,
    ) -> impl IntoElement {
        Story::container()
            .child(Story::title_for::<Disclosure>())
            .child(Story::label("Toggled"))
            .child(Disclosure::new("toggled", true))
            .child(Story::label("Not Toggled"))
            .child(Disclosure::new("not_toggled", false))
    }
}
