use std::{sync::Arc, time::Duration};

use client::{Client, UserStore};
use fs::Fs;
use gpui::{
    ease_in_out, svg, Animation, AnimationExt as _, AppContext, ClickEvent, DismissEvent,
    EventEmitter, FocusHandle, FocusableView, Model, MouseDownEvent, Render, View,
};
use language::language_settings::{AllLanguageSettings, InlineCompletionProvider};
use settings::{update_settings_file, Settings};
use ui::{prelude::*, CheckboxWithLabel, TintColor};
use workspace::{notifications::NotifyTaskExt, ModalView, Workspace};

/// Introduces user to AI inline prediction feature and terms of service
pub struct ZedPredictModal {
    user_store: Model<UserStore>,
    client: Arc<Client>,
    fs: Arc<dyn Fs>,
    focus_handle: FocusHandle,
    sign_in_status: SignInStatus,
}

#[derive(PartialEq, Eq)]
enum SignInStatus {
    /// Signed out or signed in but not from this modal
    Idle,
    /// Authentication triggered from this modal
    Waiting,
    /// Signed in after authentication from this modal
    SignedIn,
}

impl ZedPredictModal {
    fn new(
        user_store: Model<UserStore>,
        client: Arc<Client>,
        fs: Arc<dyn Fs>,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        ZedPredictModal {
            user_store,
            client,
            fs,
            focus_handle: cx.focus_handle(),
            sign_in_status: SignInStatus::Idle,
        }
    }

    pub fn toggle(
        workspace: View<Workspace>,
        user_store: Model<UserStore>,
        client: Arc<Client>,
        fs: Arc<dyn Fs>,
        cx: &mut WindowContext,
    ) {
        workspace.update(cx, |this, cx| {
            this.toggle_modal(cx, |cx| ZedPredictModal::new(user_store, client, fs, cx));
        });
    }

    fn view_terms(&mut self, _: &ClickEvent, cx: &mut ViewContext<Self>) {
        cx.open_url("https://zed.dev/terms-of-service");
        cx.notify();
    }

    fn view_blog(&mut self, _: &ClickEvent, cx: &mut ViewContext<Self>) {
        cx.open_url("https://zed.dev/blog/"); // TODO Add the link when live
        cx.notify();
    }

    fn accept_and_enable(&mut self, _: &ClickEvent, cx: &mut ViewContext<Self>) {
        let task = self
            .user_store
            .update(cx, |this, cx| this.accept_terms_of_service(cx));

        cx.spawn(|this, mut cx| async move {
            task.await?;

            this.update(&mut cx, |this, cx| {
                update_settings_file::<AllLanguageSettings>(this.fs.clone(), cx, move |file, _| {
                    file.features
                        .get_or_insert(Default::default())
                        .inline_completion_provider = Some(InlineCompletionProvider::Zed);
                });

                cx.emit(DismissEvent);
            })
        })
        .detach_and_notify_err(cx);
    }

    fn sign_in(&mut self, _: &ClickEvent, cx: &mut ViewContext<Self>) {
        let client = self.client.clone();
        self.sign_in_status = SignInStatus::Waiting;

        cx.spawn(move |this, mut cx| async move {
            let result = client.authenticate_and_connect(true, &cx).await;

            let status = match result {
                Ok(_) => SignInStatus::SignedIn,
                Err(_) => SignInStatus::Idle,
            };

            this.update(&mut cx, |this, cx| {
                this.sign_in_status = status;
                cx.notify()
            })?;

            result
        })
        .detach_and_notify_err(cx);
    }

    fn cancel(&mut self, _: &menu::Cancel, cx: &mut ViewContext<Self>) {
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for ZedPredictModal {}

impl FocusableView for ZedPredictModal {
    fn focus_handle(&self, _cx: &AppContext) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalView for ZedPredictModal {}

impl Render for ZedPredictModal {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        let base = v_flex()
            .w(px(420.))
            .p_4()
            .relative()
            .gap_2()
            .overflow_hidden()
            .elevation_3(cx)
            .id("zed predict tos")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(Self::cancel))
            .key_context("ZedPredictModal")
            .on_action(cx.listener(|_, _: &menu::Cancel, cx| {
                cx.emit(DismissEvent);
            }))
            .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, cx| {
                cx.focus(&this.focus_handle);
            }))
            .child(
                div()
                    .p_1()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(200.))
                    .child(
                        svg()
                            .path("icons/zed_predict_bg.svg")
                            .text_color(cx.theme().colors().icon_disabled)
                            .w(px(420.))
                            .h(px(128.)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .mb_2()
                    .justify_between()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new("Introducing Zed AI's")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(Headline::new("Edit Prediction").size(HeadlineSize::Large)),
                    )
                    .child({
                        let tab = |n: usize| {
                            let text_color = cx.theme().colors().text;
                            let border_color = cx.theme().colors().text_accent.opacity(0.4);

                            h_flex().child(
                                h_flex()
                                    .px_4()
                                    .py_0p5()
                                    .bg(cx.theme().colors().editor_background)
                                    .border_1()
                                    .border_color(border_color)
                                    .rounded_md()
                                    .font(theme::ThemeSettings::get_global(cx).buffer_font.clone())
                                    .text_size(TextSize::XSmall.rems(cx))
                                    .text_color(text_color)
                                    .child("tab")
                                    .with_animation(
                                        ElementId::Integer(n),
                                        Animation::new(Duration::from_secs(2)).repeat(),
                                        move |tab, delta| {
                                            let delta = (delta - 0.15 * n as f32) / 0.7;
                                            let delta = 1.0 - (0.5 - delta).abs() * 2.;
                                            let delta = ease_in_out(delta.clamp(0., 1.));
                                            let delta = 0.1 + 0.9 * delta;

                                            tab.border_color(border_color.opacity(delta))
                                                .text_color(text_color.opacity(delta))
                                        },
                                    ),
                            )
                        };

                        v_flex()
                            .gap_2()
                            .items_center()
                            .pr_4()
                            .child(tab(0).ml_neg_20())
                            .child(tab(1))
                            .child(tab(2).ml_20())
                    }),
            )
            .child(h_flex().absolute().top_2().right_2().child(
                IconButton::new("cancel", IconName::X).on_click(cx.listener(
                    |_, _: &ClickEvent, cx| {
                        cx.emit(DismissEvent);
                    },
                )),
            ));

        if self.user_store.read(cx).current_user().is_some() {
            let copy = match self.sign_in_status {
                SignInStatus::Idle => "To set Zed as your inline completions provider, ensure you:",
                SignInStatus::SignedIn => {
                    "Welcome! To set Zed as your inline completions provider, ensure you:"
                }

                SignInStatus::Waiting => unreachable!(),
            };

            base.child(Label::new(copy).color(Color::Muted))
                .child(
                    h_flex()
                        .gap_0p5()
                        .child(CheckboxWithLabel::new(
                            "tos-checkbox",
                            Label::new("Have read and accepted the").color(Color::Muted),
                            ToggleState::Unselected,
                            |_, _| {},
                        ))
                        .child(
                            Button::new("view-tos", "Terms of Service")
                                .icon(IconName::ArrowUpRight)
                                .icon_size(IconSize::Indicator)
                                .icon_color(Color::Muted)
                                .on_click(cx.listener(Self::view_terms)),
                        ),
                )
                .child(CheckboxWithLabel::new(
                    "data-checkbox",
                    Label::new("Understood that Zed AI collects completion data")
                        .color(Color::Muted),
                    ToggleState::Unselected,
                    |_, _| {},
                ))
                .child(
                    v_flex()
                        .mt_2()
                        .gap_2()
                        .w_full()
                        .child(
                            Button::new("accept-tos", "Tab to Start")
                                .style(ButtonStyle::Tinted(TintColor::Accent))
                                .full_width()
                                .on_click(cx.listener(Self::accept_and_enable)),
                        )
                        .child(
                            Button::new("blog-post", "Read the Blog Post")
                                .full_width()
                                .icon(IconName::ArrowUpRight)
                                .icon_size(IconSize::Indicator)
                                .icon_color(Color::Muted)
                                .on_click(cx.listener(Self::view_blog)),
                        ),
                )
        } else {
            base.child(
                v_flex()
                    .mt_2()
                    .gap_2()
                    .w_full()
                    .child(
                        Button::new("accept-tos", "Sign in to use Zed AI")
                            .disabled(self.sign_in_status == SignInStatus::Waiting)
                            .style(ButtonStyle::Tinted(TintColor::Accent))
                            .full_width()
                            .on_click(cx.listener(Self::sign_in)),
                    )
                    .child(
                        Button::new("blog-post", "Read the Blog Post")
                            .full_width()
                            .icon(IconName::ArrowUpRight)
                            .icon_size(IconSize::Indicator)
                            .icon_color(Color::Muted)
                            .on_click(cx.listener(Self::view_blog)),
                    ),
            )
        }
    }
}
