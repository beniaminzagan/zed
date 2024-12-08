use crate::{
    Action, AnyWindowHandle, AppCell, AppContext, AsyncAppContext, AvailableSpace,
    BackgroundExecutor, BorrowAppContext, Bounds, ClipboardItem, Context, DrawPhase, Drawable,
    Element, Empty, Entity, EventEmitter, ForegroundExecutor, Global, InputEvent, Keystroke, Model,
    Modifiers, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Platform, Point, Render, Result, Size, Task, TestDispatcher, TestPlatform, TestWindow,
    TextSystem, Window, WindowBounds, WindowHandle, WindowOptions,
};
use anyhow::{anyhow, bail};
use futures::{channel::oneshot, Stream, StreamExt};
use std::{cell::RefCell, future::Future, rc::Rc, sync::Arc, time::Duration};

/// A TestAppContext is provided to tests created with `#[gpui::test]`, it provides
/// an implementation of `Context` with additional methods that are useful in tests.
#[derive(Clone)]
pub struct TestAppContext {
    #[doc(hidden)]
    pub app: Rc<AppCell>,
    #[doc(hidden)]
    pub background_executor: BackgroundExecutor,
    #[doc(hidden)]
    pub foreground_executor: ForegroundExecutor,
    #[doc(hidden)]
    pub dispatcher: TestDispatcher,
    test_platform: Rc<TestPlatform>,
    text_system: Arc<TextSystem>,
    fn_name: Option<&'static str>,
    on_quit: Rc<RefCell<Vec<Box<dyn FnOnce() + 'static>>>>,
}

impl Context for TestAppContext {
    type Result<T> = T;

    fn new_model<T: 'static>(
        &mut self,
        build_model: impl FnOnce(&Model<T>, &mut AppContext) -> T,
    ) -> Self::Result<Model<T>> {
        let mut app = self.app.borrow_mut();
        app.new_model(build_model)
    }

    fn reserve_model<T: 'static>(&mut self) -> Self::Result<crate::Reservation<T>> {
        let mut app = self.app.borrow_mut();
        app.reserve_model()
    }

    fn insert_model<T: 'static>(
        &mut self,
        reservation: crate::Reservation<T>,
        build_model: impl FnOnce(&Model<T>, &mut AppContext) -> T,
    ) -> Self::Result<Model<T>> {
        let mut app = self.app.borrow_mut();
        app.insert_model(reservation, build_model)
    }

    fn update_model<T: 'static, R>(
        &mut self,
        handle: &Model<T>,
        update: impl FnOnce(&mut T, &Model<T>, &mut AppContext) -> R,
    ) -> Self::Result<R> {
        let mut app = self.app.borrow_mut();
        app.update_model(handle, update)
    }

    fn read_model<T, R>(
        &self,
        handle: &Model<T>,
        read: impl FnOnce(&T, &Model<T>, &AppContext) -> R,
    ) -> Self::Result<R>
    where
        T: 'static,
    {
        let app = self.app.borrow();
        app.read_model(handle, read)
    }

    fn update_window<T, F>(&mut self, window: AnyWindowHandle, f: F) -> Result<T>
    where
        F: FnOnce(&mut Window, &mut AppContext) -> T,
    {
        let mut lock = self.app.borrow_mut();
        lock.update_window(window, f)
    }

    fn read_window<R>(
        &self,
        window: AnyWindowHandle,
        read: impl FnOnce(&Window, &AppContext) -> R,
    ) -> Result<R> {
        let app = self.app.borrow();
        app.read_window(window, read)
    }
}

impl TestAppContext {
    /// Creates a new `TestAppContext`. Usually you can rely on `#[gpui::test]` to do this for you.
    pub fn new(dispatcher: TestDispatcher, fn_name: Option<&'static str>) -> Self {
        let arc_dispatcher = Arc::new(dispatcher.clone());
        let background_executor = BackgroundExecutor::new(arc_dispatcher.clone());
        let foreground_executor = ForegroundExecutor::new(arc_dispatcher);
        let platform = TestPlatform::new(background_executor.clone(), foreground_executor.clone());
        let asset_source = Arc::new(());
        let http_client = http_client::FakeHttpClient::with_404_response();
        let text_system = Arc::new(TextSystem::new(platform.text_system()));

        Self {
            app: AppContext::new(platform.clone(), asset_source, http_client),
            background_executor,
            foreground_executor,
            dispatcher: dispatcher.clone(),
            test_platform: platform,
            text_system,
            fn_name,
            on_quit: Rc::new(RefCell::new(Vec::default())),
        }
    }

    /// The name of the test function that created this `TestAppContext`
    pub fn test_function_name(&self) -> Option<&'static str> {
        self.fn_name
    }

    /// Checks whether there have been any new path prompts received by the platform.
    pub fn did_prompt_for_new_path(&self) -> bool {
        self.test_platform.did_prompt_for_new_path()
    }

    /// returns a new `TestAppContext` re-using the same executors to interleave tasks.
    pub fn new_app(&self) -> TestAppContext {
        Self::new(self.dispatcher.clone(), self.fn_name)
    }

    /// Called by the test helper to end the test.
    /// public so the macro can call it.
    pub fn quit(&self) {
        self.on_quit.borrow_mut().drain(..).for_each(|f| f());
        self.app.borrow_mut().shutdown();
    }

    /// Register cleanup to run when the test ends.
    pub fn on_quit(&mut self, f: impl FnOnce() + 'static) {
        self.on_quit.borrow_mut().push(Box::new(f));
    }

    /// Schedules all windows to be redrawn on the next effect cycle.
    pub fn refresh(&mut self) -> Result<()> {
        let mut app = self.app.borrow_mut();
        app.refresh();
        Ok(())
    }

    /// Returns an executor (for running tasks in the background)
    pub fn executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }

    /// Returns an executor (for running tasks on the main thread)
    pub fn foreground_executor(&self) -> &ForegroundExecutor {
        &self.foreground_executor
    }

    /// Gives you an `&mut AppContext` for the duration of the closure
    pub fn update<R>(&self, f: impl FnOnce(&mut AppContext) -> R) -> R {
        let mut cx = self.app.borrow_mut();
        cx.update(f)
    }

    /// Gives you an `&AppContext` for the duration of the closure
    pub fn read<R>(&self, f: impl FnOnce(&AppContext) -> R) -> R {
        let cx = self.app.borrow();
        f(&cx)
    }

    /// Adds a new window. The Window will always be backed by a `TestWindow` which
    /// can be retrieved with `self.test_window(handle)`
    pub fn add_window<T: 'static + Render>(
        &mut self,
        builder: impl FnOnce(&Model<T>, &mut Window, &mut AppContext) -> T,
    ) -> WindowHandle<T> {
        let mut cx = self.app.borrow_mut();

        // Some tests rely on the window size matching the bounds of the test display
        let bounds = Bounds::maximized(None, &mut cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            builder,
        )
        .unwrap()
    }

    /// Adds a new window with no content.
    pub fn add_empty_window(&mut self) -> &mut VisualTestContext {
        let mut cx = self.app.borrow_mut();
        let bounds = Bounds::maximized(None, &mut cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_window, _model, _cx| Empty,
            )
            .unwrap();
        drop(cx);
        let cx = VisualTestContext::from_window(window.into(), self).as_mut();
        cx.run_until_parked();
        cx
    }

    /// Adds a new window, and returns its root view and a `VisualTestContext` which can be used
    /// as a `WindowContext` for the rest of the test. Typically you would shadow this context with
    /// the returned one. `let (view, cx) = cx.add_window_view(...);`
    pub fn add_window_view<T, F>(&mut self, builder: F) -> (Model<T>, &mut VisualTestContext)
    where
        T: 'static + Render,
        F: FnOnce(&Model<T>, &mut Window, &mut AppContext) -> T,
    {
        let mut cx = self.app.borrow_mut();
        let bounds = Bounds::maximized(None, &mut cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                builder,
            )
            .unwrap();
        drop(cx);
        let state = window.state(self).unwrap();
        let cx = VisualTestContext::from_window(window.into(), self).as_mut();
        cx.run_until_parked();

        // it might be nice to try and cleanup these at the end of each test.
        (state, cx)
    }

    /// returns the TextSystem
    pub fn text_system(&self) -> &Arc<TextSystem> {
        &self.text_system
    }

    /// Simulates writing to the platform clipboard
    pub fn write_to_clipboard(&self, item: ClipboardItem) {
        self.test_platform.write_to_clipboard(item)
    }

    /// Simulates reading from the platform clipboard.
    /// This will return the most recent value from `write_to_clipboard`.
    pub fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.test_platform.read_from_clipboard()
    }

    /// Simulates choosing a File in the platform's "Open" dialog.
    pub fn simulate_new_path_selection(
        &self,
        select_path: impl FnOnce(&std::path::Path) -> Option<std::path::PathBuf>,
    ) {
        self.test_platform.simulate_new_path_selection(select_path);
    }

    /// Simulates clicking a button in an platform-level alert dialog.
    pub fn simulate_prompt_answer(&self, button_ix: usize) {
        self.test_platform.simulate_prompt_answer(button_ix);
    }

    /// Returns true if there's an alert dialog open.
    pub fn has_pending_prompt(&self) -> bool {
        self.test_platform.has_pending_prompt()
    }

    /// All the urls that have been opened with cx.open_url() during this test.
    pub fn opened_url(&self) -> Option<String> {
        self.test_platform.opened_url.borrow().clone()
    }

    /// Simulates the user resizing the window to the new size.
    pub fn simulate_window_resize(&self, window_handle: AnyWindowHandle, size: Size<Pixels>) {
        self.test_window(window_handle).simulate_resize(size);
    }

    /// Returns all windows open in the test.
    pub fn windows(&self) -> Vec<AnyWindowHandle> {
        self.app.borrow().windows().clone()
    }

    /// Run the given task on the main thread.
    pub fn spawn<Fut, R>(&self, f: impl FnOnce(AsyncAppContext) -> Fut) -> Task<R>
    where
        Fut: Future<Output = R> + 'static,
        R: 'static,
    {
        self.foreground_executor.spawn(f(self.to_async()))
    }

    /// true if the given global is defined
    pub fn has_global<G: Global>(&self) -> bool {
        let app = self.app.borrow();
        app.has_global::<G>()
    }

    /// runs the given closure with a reference to the global
    /// panics if `has_global` would return false.
    pub fn read_global<G: Global, R>(&self, read: impl FnOnce(&G, &AppContext) -> R) -> R {
        let app = self.app.borrow();
        read(app.global(), &app)
    }

    /// runs the given closure with a reference to the global (if set)
    pub fn try_read_global<G: Global, R>(
        &self,
        read: impl FnOnce(&G, &AppContext) -> R,
    ) -> Option<R> {
        let lock = self.app.borrow();
        Some(read(lock.try_global()?, &lock))
    }

    /// sets the global in this context.
    pub fn set_global<G: Global>(&mut self, global: G) {
        let mut lock = self.app.borrow_mut();
        lock.update(|cx| cx.set_global(global))
    }

    /// updates the global in this context. (panics if `has_global` would return false)
    pub fn update_global<G: Global, R>(
        &mut self,
        update: impl FnOnce(&mut G, &mut AppContext) -> R,
    ) -> R {
        let mut lock = self.app.borrow_mut();
        lock.update(|cx| cx.update_global(update))
    }

    /// Returns an `AsyncAppContext` which can be used to run tasks that expect to be on a background
    /// thread on the current thread in tests.
    pub fn to_async(&self) -> AsyncAppContext {
        AsyncAppContext {
            app: Rc::downgrade(&self.app),
            background_executor: self.background_executor.clone(),
            foreground_executor: self.foreground_executor.clone(),
        }
    }

    /// Wait until there are no more pending tasks.
    pub fn run_until_parked(&mut self) {
        self.background_executor.run_until_parked()
    }

    /// Simulate dispatching an action to the currently focused node in the window.
    pub fn dispatch_action<A>(&mut self, window: AnyWindowHandle, action: A)
    where
        A: Action,
    {
        window
            .update(self, |window, cx| {
                window.dispatch_action(action.boxed_clone(), cx)
            })
            .unwrap();

        self.background_executor.run_until_parked()
    }

    /// simulate_keystrokes takes a space-separated list of keys to type.
    /// cx.simulate_keystrokes("cmd-shift-p b k s p enter")
    /// in Zed, this will run backspace on the current editor through the command palette.
    /// This will also run the background executor until it's parked.
    pub fn simulate_keystrokes(&mut self, window: AnyWindowHandle, keystrokes: &str) {
        for keystroke in keystrokes
            .split(' ')
            .map(Keystroke::parse)
            .map(Result::unwrap)
        {
            self.dispatch_keystroke(window, keystroke);
        }

        self.background_executor.run_until_parked()
    }

    /// simulate_input takes a string of text to type.
    /// cx.simulate_input("abc")
    /// will type abc into your current editor
    /// This will also run the background executor until it's parked.
    pub fn simulate_input(&mut self, window: AnyWindowHandle, input: &str) {
        for keystroke in input.split("").map(Keystroke::parse).map(Result::unwrap) {
            self.dispatch_keystroke(window, keystroke);
        }

        self.background_executor.run_until_parked()
    }

    /// dispatches a single Keystroke (see also `simulate_keystrokes` and `simulate_input`)
    pub fn dispatch_keystroke(&mut self, window: AnyWindowHandle, keystroke: Keystroke) {
        self.update_window(window, |window, cx| {
            window.dispatch_keystroke(keystroke, cx)
        })
        .unwrap();
    }

    /// Returns the `TestWindow` backing the given handle.
    pub(crate) fn test_window(&self, window: AnyWindowHandle) -> TestWindow {
        self.app
            .borrow_mut()
            .windows
            .get_mut(window.id)
            .unwrap()
            .as_mut()
            .unwrap()
            .platform_window
            .as_test()
            .unwrap()
            .clone()
    }

    /// Returns a stream of notifications whenever the View or Model is updated.
    pub fn notifications<T: 'static>(&mut self, entity: &impl Entity<T>) -> impl Stream<Item = ()> {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        self.update(|cx| {
            cx.observe(entity, {
                let tx = tx.clone();
                move |_, _| {
                    let _ = tx.unbounded_send(());
                }
            })
            .detach();
            cx.observe_release(entity, move |_, _| tx.close_channel())
                .detach()
        });
        rx
    }

    /// Returns a stream of events emitted by the given Model.
    pub fn events<Evt, T: 'static + EventEmitter<Evt>>(
        &mut self,
        entity: &Model<T>,
    ) -> futures::channel::mpsc::UnboundedReceiver<Evt>
    where
        Evt: 'static + Clone,
    {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        entity
            .update(self, |_, _model: &Model<T>, cx: &mut AppContext| {
                cx.subscribe(entity, move |_handle, event, _cx| {
                    let _ = tx.unbounded_send(event.clone());
                })
            })
            .detach();
        rx
    }

    /// Runs until the given condition becomes true. (Prefer `run_until_parked` if you
    /// don't need to jump in at a specific time).
    pub async fn condition<T: 'static>(
        &mut self,
        model: &Model<T>,
        mut predicate: impl FnMut(&mut T, &Model<T>, &mut AppContext) -> bool,
    ) {
        let timer = self.executor().timer(Duration::from_secs(3));
        let mut notifications = self.notifications(model);

        use futures::FutureExt as _;
        use smol::future::FutureExt as _;

        async {
            loop {
                if model.update(self, &mut predicate) {
                    return Ok(());
                }

                if notifications.next().await.is_none() {
                    bail!("model dropped")
                }
            }
        }
        .race(timer.map(|_| Err(anyhow!("condition timed out"))))
        .await
        .unwrap();
    }

    /// Set a name for this App.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_name(&mut self, name: &'static str) {
        self.update(|cx| cx.name = Some(name))
    }
}

impl<T: 'static> Model<T> {
    /// Block until the next event is emitted by the model, then return it.
    pub fn next_event<Event>(&self, cx: &mut TestAppContext) -> impl Future<Output = Event>
    where
        Event: Send + Clone + 'static,
        T: EventEmitter<Event>,
    {
        let (tx, mut rx) = oneshot::channel();
        let mut tx = Some(tx);
        let subscription = self.update(cx, |_, _, cx| {
            cx.subscribe(self, move |_, event, _| {
                if let Some(tx) = tx.take() {
                    _ = tx.send(event.clone());
                }
            })
        });

        async move {
            let event = rx.await.expect("no event emitted");
            drop(subscription);
            event
        }
    }

    /// Returns a future that resolves when the model notifies.
    pub fn next_notification(&self, cx: &TestAppContext) -> impl Future<Output = ()> {
        use postage::prelude::{Sink as _, Stream as _};

        let (mut tx, mut rx) = postage::mpsc::channel(1);
        let mut cx = cx.app.app.borrow_mut();
        let subscription = cx.observe(self, move |_, _| {
            tx.try_send(()).ok();
        });

        let duration = if std::env::var("CI").is_ok() {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(1)
        };

        async move {
            let notification = crate::util::timeout(duration, rx.recv())
                .await
                .expect("next notification timed out");
            drop(subscription);
            notification.expect("model dropped while test was waiting for its next notification")
        }
    }
}

// impl<V: 'static> View<V> {
//     /// Returns a future that resolves when the view is next updated.
//     pub fn next_notification(&self, cx: &TestAppContext) -> impl Future<Output = ()> {
//         use postage::prelude::{Sink as _, Stream as _};

//         let (mut tx, mut rx) = postage::mpsc::channel(1);
//         let mut cx = cx.app.app.borrow_mut();
//         let subscription = cx.observe(self, move |_, _| {
//             tx.try_send(()).ok();
//         });

//         let duration = if std::env::var("CI").is_ok() {
//             Duration::from_secs(5)
//         } else {
//             Duration::from_secs(1)
//         };

//         async move {
//             let notification = crate::util::timeout(duration, rx.recv())
//                 .await
//                 .expect("next notification timed out");
//             drop(subscription);
//             notification.expect("model dropped while test was waiting for its next notification")
//         }
//     }
// }

// impl<V> View<V> {
//     /// Returns a future that resolves when the condition becomes true.
//     pub fn condition<Evt>(
//         &self,
//         cx: &TestAppContext,
//         mut predicate: impl FnMut(&V, &AppContext) -> bool,
//     ) -> impl Future<Output = ()>
//     where
//         Evt: 'static,
//         V: EventEmitter<Evt>,
//     {
//         use postage::prelude::{Sink as _, Stream as _};

//         let (tx, mut rx) = postage::mpsc::channel(1024);
//         let timeout_duration = if cfg!(target_os = "macos") {
//             Duration::from_millis(100)
//         } else {
//             Duration::from_secs(1)
//         };

//         let mut cx = cx.app.borrow_mut();
//         let subscriptions = (
//             cx.observe(self, {
//                 let mut tx = tx.clone();
//                 move |_, _| {
//                     tx.blocking_send(()).ok();
//                 }
//             }),
//             cx.subscribe(self, {
//                 let mut tx = tx.clone();
//                 move |_, _: &Evt, _| {
//                     tx.blocking_send(()).ok();
//                 }
//             }),
//         );

//         let cx = cx.this.upgrade().unwrap();
//         let handle = self.downgrade();

//         async move {
//             crate::util::timeout(timeout_duration, async move {
//                 loop {
//                     {
//                         let cx = cx.borrow();
//                         let cx = &*cx;
//                         if predicate(
//                             handle
//                                 .upgrade()
//                                 .expect("view dropped with pending condition")
//                                 .read(cx),
//                             cx,
//                         ) {
//                             break;
//                         }
//                     }

//                     cx.borrow().background_executor().start_waiting();
//                     rx.recv()
//                         .await
//                         .expect("view dropped with pending condition");
//                     cx.borrow().background_executor().finish_waiting();
//                 }
//             })
//             .await
//             .expect("condition timed out");
//             drop(subscriptions);
//         }
//     }
// }

use derive_more::{Deref, DerefMut};
#[derive(Deref, DerefMut, Clone)]
/// A VisualTestContext is the test-equivalent of a `WindowContext`. It allows you to
/// run window-specific test code.
pub struct VisualTestContext {
    #[deref]
    #[deref_mut]
    /// cx is the original TestAppContext (you can more easily access this using Deref)
    pub cx: TestAppContext,
    window: AnyWindowHandle,
}

impl VisualTestContext {
    /// Get the underlying window handle underlying this context.
    pub fn handle(&self) -> AnyWindowHandle {
        self.window
    }

    /// Provides the `WindowContext` for the duration of the closure.
    pub fn update<R>(&mut self, f: impl FnOnce(&mut Window, &mut AppContext) -> R) -> R {
        self.cx
            .update_window(self.window, |window, cx| f(window, cx))
            .unwrap()
    }

    /// Creates a new VisualTestContext. You would typically shadow the passed in
    /// TestAppContext with this, as this is typically more useful.
    /// `let cx = VisualTestContext::from_window(window, cx);`
    pub fn from_window(window: AnyWindowHandle, cx: &TestAppContext) -> Self {
        Self {
            cx: cx.clone(),
            window,
        }
    }

    /// Wait until there are no more pending tasks.
    pub fn run_until_parked(&self) {
        self.cx.background_executor.run_until_parked();
    }

    /// Dispatch the action to the currently focused node.
    pub fn dispatch_action<A>(&mut self, action: A)
    where
        A: Action,
    {
        self.cx.dispatch_action(self.window, action)
    }

    /// Read the title off the window (set by `WindowContext#set_window_title`)
    pub fn window_title(&mut self) -> Option<String> {
        self.cx.test_window(self.window).0.lock().title.clone()
    }

    /// Simulate a sequence of keystrokes `cx.simulate_keystrokes("cmd-p escape")`
    /// Automatically runs until parked.
    pub fn simulate_keystrokes(&mut self, keystrokes: &str) {
        self.cx.simulate_keystrokes(self.window, keystrokes)
    }

    /// Simulate typing text `cx.simulate_input("hello")`
    /// Automatically runs until parked.
    pub fn simulate_input(&mut self, input: &str) {
        self.cx.simulate_input(self.window, input)
    }

    /// Simulate a mouse move event to the given point
    pub fn simulate_mouse_move(
        &mut self,
        position: Point<Pixels>,
        button: impl Into<Option<MouseButton>>,
        modifiers: Modifiers,
    ) {
        self.simulate_event(MouseMoveEvent {
            position,
            modifiers,
            pressed_button: button.into(),
        })
    }

    /// Simulate a mouse down event to the given point
    pub fn simulate_mouse_down(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        modifiers: Modifiers,
    ) {
        self.simulate_event(MouseDownEvent {
            position,
            modifiers,
            button,
            click_count: 1,
            first_mouse: false,
        })
    }

    /// Simulate a mouse up event to the given point
    pub fn simulate_mouse_up(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        modifiers: Modifiers,
    ) {
        self.simulate_event(MouseUpEvent {
            position,
            modifiers,
            button,
            click_count: 1,
        })
    }

    /// Simulate a primary mouse click at the given point
    pub fn simulate_click(&mut self, position: Point<Pixels>, modifiers: Modifiers) {
        self.simulate_event(MouseDownEvent {
            position,
            modifiers,
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        self.simulate_event(MouseUpEvent {
            position,
            modifiers,
            button: MouseButton::Left,
            click_count: 1,
        });
    }

    /// Simulate a modifiers changed event
    pub fn simulate_modifiers_change(&mut self, modifiers: Modifiers) {
        self.simulate_event(ModifiersChangedEvent { modifiers })
    }

    /// Simulates the user resizing the window to the new size.
    pub fn simulate_resize(&self, size: Size<Pixels>) {
        self.simulate_window_resize(self.window, size)
    }

    /// debug_bounds returns the bounds of the element with the given selector.
    pub fn debug_bounds(&mut self, selector: &'static str) -> Option<Bounds<Pixels>> {
        self.update(|window, _cx| window.rendered_frame.debug_bounds.get(selector).copied())
    }

    /// Draw an element to the window. Useful for simulating events or actions
    pub fn draw<E>(
        &mut self,
        origin: Point<Pixels>,
        space: impl Into<Size<AvailableSpace>>,
        f: impl FnOnce(&mut Window, &mut AppContext) -> E,
    ) -> (E::RequestLayoutState, E::PrepaintState)
    where
        E: Element,
    {
        self.update(|window, cx| {
            window.draw_phase = DrawPhase::Prepaint;
            let mut element = Drawable::new(f(window, cx));
            element.layout_as_root(space.into(), window, cx);
            window.with_absolute_element_offset(origin, |window| element.prepaint(window, cx));

            window.draw_phase = DrawPhase::Paint;
            let (request_layout_state, prepaint_state) = element.paint(window, cx);

            window.draw_phase = DrawPhase::None;
            cx.refresh();

            (request_layout_state, prepaint_state)
        })
    }

    /// Simulate an event from the platform, e.g. a SrollWheelEvent
    /// Make sure you've called [VisualTestContext::draw] first!
    pub fn simulate_event<E: InputEvent>(&mut self, event: E) {
        self.test_window(self.window)
            .simulate_input(event.to_platform_input());
        self.background_executor.run_until_parked();
    }

    /// Simulates the user blurring the window.
    pub fn deactivate_window(&mut self) {
        if Some(self.window) == self.test_platform.active_window() {
            self.test_platform.set_active_window(None)
        }
        self.background_executor.run_until_parked();
    }

    /// Simulates the user closing the window.
    /// Returns true if the window was closed.
    pub fn simulate_close(&mut self) -> bool {
        let handler = self
            .window
            .update(&mut self.cx, |window, _cx| {
                window
                    .platform_window
                    .as_test()
                    .unwrap()
                    .0
                    .lock()
                    .should_close_handler
                    .take()
            })
            .unwrap();

        if let Some(mut handler) = handler {
            let should_close = handler();
            self.window
                .update(&mut self.cx, |window, _cx| {
                    window.platform_window.on_should_close(handler);
                })
                .unwrap();
            should_close
        } else {
            false
        }
    }

    /// Get an &mut VisualTestContext (which is mostly what you need to pass to other methods).
    /// This method internally retains the VisualTestContext until the end of the test.
    pub fn as_mut(self) -> &'static mut Self {
        let ptr = Box::into_raw(Box::new(self));
        // safety: on_quit will be called after the test has finished.
        // the executor will ensure that all tasks related to the test have stopped.
        // so there is no way for cx to be accessed after on_quit is called.
        let cx = Box::leak(unsafe { Box::from_raw(ptr) });
        cx.on_quit(move || unsafe {
            drop(Box::from_raw(ptr));
        });
        cx
    }
}

impl Context for VisualTestContext {
    type Result<T> = <TestAppContext as Context>::Result<T>;

    fn new_model<T: 'static>(
        &mut self,
        build_model: impl FnOnce(&Model<T>, &mut AppContext) -> T,
    ) -> Self::Result<Model<T>> {
        self.cx.new_model(build_model)
    }

    fn reserve_model<T: 'static>(&mut self) -> Self::Result<crate::Reservation<T>> {
        self.cx.reserve_model()
    }

    fn insert_model<T: 'static>(
        &mut self,
        reservation: crate::Reservation<T>,
        build_model: impl FnOnce(&Model<T>, &mut AppContext) -> T,
    ) -> Self::Result<Model<T>> {
        self.cx.insert_model(reservation, build_model)
    }

    fn update_model<T, R>(
        &mut self,
        handle: &Model<T>,
        update: impl FnOnce(&mut T, &Model<T>, &mut AppContext) -> R,
    ) -> Self::Result<R>
    where
        T: 'static,
    {
        self.cx.update_model(handle, update)
    }

    fn read_model<T, R>(
        &self,
        handle: &Model<T>,
        read: impl FnOnce(&T, &Model<T>, &AppContext) -> R,
    ) -> Self::Result<R>
    where
        T: 'static,
    {
        self.cx.read_model(handle, read)
    }

    fn update_window<T, F>(&mut self, window: AnyWindowHandle, f: F) -> Result<T>
    where
        F: FnOnce(&mut Window, &mut AppContext) -> T,
    {
        self.cx.update_window(window, f)
    }

    fn read_window<R>(
        &self,
        window: AnyWindowHandle,
        read: impl FnOnce(&Window, &AppContext) -> R,
    ) -> Result<R> {
        self.cx.read_window(window, read)
    }
}
