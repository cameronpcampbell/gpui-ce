use crate::{
    AnyElement, ElementId, IntoElement, IntoItemMatch, IntoListMatch, IntoMatchValues, ItemMatch,
    ListMatch, SharedString,
    reflection::{ReflectedElement, ReflectedTrait, ReflectedTraits, ReflectionGroup},
};
use smallvec::SmallVec;
use std::{
    cell::{Cell, RefCell},
    marker::PhantomData,
    num::NonZeroUsize,
    rc::Rc,
};

/// The reflection group used by selectors without a reflected trait predicate.
#[doc(hidden)]
pub struct NoReflectedTraits;

impl ReflectionGroup for NoReflectedTraits {}

/// Selects elements relative to an element tree root.
pub struct Select<Group = NoReflectedTraits>
where
    Group: ReflectionGroup,
{
    matcher: SelectorMatcher,
    group: PhantomData<fn() -> Group>,
}

struct SelectorMatcher {
    scope: SelectScope,
    reflected_traits: SmallVec<[ReflectedTrait; 2]>,
    element_id: Option<ItemMatch<ElementId>>,
    classes: SmallVec<[ListMatch<SharedString>; 2]>,
    nth: Option<usize>,
    every: Option<NonZeroUsize>,
}

#[derive(Clone, Copy)]
enum SelectScope {
    This,
    Children,
    Descendants,
}

impl Select<NoReflectedTraits> {
    /// Selects the element on which the selector is installed.
    pub fn this() -> Self {
        Self::new(SelectScope::This)
    }

    /// Selects direct children of the element on which the selector is installed.
    pub fn children() -> Self {
        Self::new(SelectScope::Children)
    }

    /// Selects every descendant of the element on which the selector is installed.
    pub fn descendants() -> Self {
        Self::new(SelectScope::Descendants)
    }

    fn new(scope: SelectScope) -> Self {
        Self {
            matcher: SelectorMatcher {
                scope,
                reflected_traits: SmallVec::new(),
                element_id: None,
                classes: SmallVec::new(),
                nth: None,
                every: None,
            },
            group: PhantomData,
        }
    }

    /// Requires matching elements to reflect every trait in the given trait set.
    /// Borrowed callback methods honor concrete defaults and overrides unless marked
    /// `#[reflect(wrapper_default)]`.
    pub fn reflects<Traits>(mut self, reflected_traits: Traits) -> Select<Traits::Group>
    where
        Traits: ReflectedTraits,
    {
        self.matcher
            .reflected_traits
            .extend(reflected_traits.reflected_traits());

        Select {
            matcher: self.matcher,
            group: PhantomData,
        }
    }
}

impl<Group> Select<Group>
where
    Group: ReflectionGroup,
{
    /// Matches one element ID, optionally negated with [`crate::not`].
    ///
    /// Composite IDs such as `("row", 3_usize)` remain one ID. A negated ID also
    /// matches elements without an ID. Calling this method again replaces the ID condition.
    ///
    /// ```compile_fail
    /// use gpui::Select;
    ///
    /// let selector = Select::children().id(["save", "cancel"]);
    /// ```
    ///
    /// ```compile_fail
    /// use gpui::{Select, any};
    ///
    /// let selector = Select::children().id(any(["save", "cancel"]));
    /// ```
    pub fn id<Kind>(mut self, element_id: impl IntoItemMatch<ElementId, Kind>) -> Self {
        self.matcher.element_id = Some(element_id.into_item_match());

        self
    }

    /// Matches class membership using values, arrays, vectors, slices, or tuples.
    ///
    /// Collections require every expression to match. Use [`crate::any`] for OR,
    /// and [`crate::not`] to negate an expression. Repeated calls require all conditions.
    ///
    /// ```
    /// use gpui::{Select, any, not};
    ///
    /// let selector = Select::children().class(("apple", "pear", not("plum")));
    /// let selector = Select::children().class(not(any(["apple", "pear"])));
    /// ```
    pub fn class<Kind>(mut self, class: impl IntoListMatch<SharedString, Kind>) -> Self {
        self.matcher.classes.push(class.into_list_match());

        self
    }

    /// Selects the matching element at a zero-based index in layout order.
    ///
    /// Measurements are excluded, and virtualized lists count only rendered elements.
    /// Prefer color changes over size changes when selecting list rows by position.
    pub fn nth(mut self, idx: usize) -> Self {
        self.matcher.nth = Some(idx);

        self
    }

    /// Selects matching elements at indexes `0, step, 2 * step, ...`.
    ///
    /// Uses the same indexing rules as [`Self::nth`].
    ///
    /// # Panics
    ///
    /// Panics if `step` is zero.
    pub fn every(mut self, step: usize) -> Self {
        self.matcher.every =
            Some(NonZeroUsize::new(step).expect("selector interval must be nonzero"));

        self
    }

    fn into_matcher(self) -> SelectorMatcher {
        self.matcher
    }
}

impl SelectorMatcher {
    fn matches_predicates(&self, element: &AnyElement) -> bool {
        if !self
            .reflected_traits
            .iter()
            .all(|reflected_trait| element.implements_trait(*reflected_trait))
        {
            return false;
        }

        if let Some(element_id) = self.element_id.as_ref()
            && !element_id.evaluate(&|expected| element.element_id().as_ref() == Some(expected))
        {
            return false;
        }

        if !self
            .classes
            .iter()
            .all(|class| class.matches(element.classes()))
        {
            return false;
        }

        true
    }

    fn has_position(&self) -> bool {
        self.nth.is_some() || self.every.is_some()
    }

    fn includes_position(&self, idx: usize) -> bool {
        self.nth.is_none_or(|expected| idx == expected)
            && self.every.is_none_or(|step| idx.is_multiple_of(step.get()))
    }

    fn includes_depth(&self, depth: usize) -> bool {
        match self.scope {
            SelectScope::This => depth == 0,
            SelectScope::Children => depth == 1,
            SelectScope::Descendants => depth >= 1,
        }
    }

    fn can_reach_view_contents(&self, depth: usize) -> bool {
        debug_assert!(depth >= 1);

        match self.scope {
            SelectScope::This => false,
            SelectScope::Children => depth == 1,
            SelectScope::Descendants => true,
        }
    }

    fn can_select_future_match(&self, next_match_idx: usize, measuring: bool) -> bool {
        if measuring && self.has_position() {
            return false;
        }

        self.nth.is_none_or(|expected| next_match_idx <= expected)
    }
}

/// Adds selector metadata and transformations to elements.
pub trait SelectableElement: IntoElement + Sized {
    /// Tags this element with one class or a collection of positive class names.
    ///
    /// Arrays, vectors, slices, and tuples are supported. Boolean expressions are
    /// only accepted by [`Select::class`].
    ///
    /// ```compile_fail
    /// use gpui::{SelectableElement, div, not};
    ///
    /// let element = div().class(not("apple"));
    /// ```
    fn class<Kind>(self, class: impl IntoMatchValues<SharedString, Kind>) -> AnyElement {
        let mut element = self.into_any_element();
        class.extend_match_values(element.classes_mut());

        element
    }

    /// Transforms matching elements before layout.
    ///
    /// The callback may run for measurements or retries, so keep results repeatable
    /// and avoid changing application state.
    fn select<Group, Output>(
        self,
        selector: Select<Group>,
        mut transform: impl FnMut(ReflectedElement<Group>) -> Output + 'static,
    ) -> AnyElement
    where
        Group: ReflectionGroup,
        Output: IntoElement + 'static,
    {
        let mut element = self.into_any_element();
        let transform = move |element| transform(ReflectedElement::new(element)).into_any_element();
        element.add_selector(PendingSelector::new(
            selector.into_matcher(),
            Box::new(transform),
        ));

        element
    }
}

impl<ElementType> SelectableElement for ElementType where ElementType: IntoElement {}

pub(crate) struct ElementMetadata {
    pub(crate) classes: SmallVec<[SharedString; 2]>,
    pub(crate) selectors: Vec<PendingSelector>,
    pub(crate) node_state: Option<SelectorNodeState>,
}

impl ElementMetadata {
    pub(crate) fn new() -> Self {
        Self {
            classes: SmallVec::new(),
            selectors: Vec::new(),
            node_state: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectorRuleId(u64);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct LayoutAttemptId(u64);

struct SelectorVisit {
    rule: SelectorRuleId,
    attempt: LayoutAttemptId,
}

#[derive(Default)]
pub(crate) struct SelectorNodeState {
    visited: SmallVec<[SelectorVisit; 2]>,
    pub(crate) generated_by: SmallVec<[SelectorRuleId; 2]>,
}

impl SelectorNodeState {
    pub(crate) fn generated(generated_by: SmallVec<[SelectorRuleId; 2]>) -> Self {
        Self {
            generated_by,
            ..Self::default()
        }
    }

    fn record_visit(&mut self, rule: SelectorRuleId, attempt: LayoutAttemptId) -> bool {
        self.visited.retain(|visit| visit.attempt == attempt);

        if self.visited.iter().any(|visit| visit.rule == rule) {
            return false;
        }

        self.visited.push(SelectorVisit { rule, attempt });

        true
    }
}

type SelectorTransform = Box<dyn FnMut(AnyElement) -> AnyElement>;

#[derive(Clone)]
pub(crate) struct PendingSelector(Rc<SelectorRule>);

impl PendingSelector {
    fn new(matcher: SelectorMatcher, transform: SelectorTransform) -> Self {
        let identity = NEXT_SELECTOR_RULE_ID.with(|next| {
            let identity = next.get();
            next.set(
                identity
                    .checked_add(1)
                    .expect("selector rule IDs exhausted"),
            );

            SelectorRuleId(identity)
        });

        Self(Rc::new(SelectorRule {
            identity,
            matcher,
            transform: RefCell::new(Some(transform)),
            next_match_idx: Cell::new(0),
        }))
    }

    pub(crate) fn identity(&self) -> SelectorRuleId {
        self.0.identity
    }
}

struct SelectorRule {
    identity: SelectorRuleId,
    matcher: SelectorMatcher,
    transform: RefCell<Option<SelectorTransform>>,
    next_match_idx: Cell<usize>,
}

struct SelectorCallbackLease<'rule> {
    rule: &'rule SelectorRule,
    transform: Option<SelectorTransform>,
}

impl SelectorRule {
    fn try_lease(&self) -> Option<SelectorCallbackLease<'_>> {
        let transform = self.transform.borrow_mut().take()?;

        Some(SelectorCallbackLease {
            rule: self,
            transform: Some(transform),
        })
    }
}

impl Drop for SelectorCallbackLease<'_> {
    fn drop(&mut self) {
        let transform = self
            .transform
            .take()
            .expect("selector callback lease is empty");
        self.rule.transform.borrow_mut().replace(transform);
    }
}

#[derive(Clone)]
struct SelectorBinding {
    rule: Rc<SelectorRule>,
    depth: usize,
}

#[derive(Clone, Default)]
pub(crate) struct SelectorContext {
    bindings: SmallVec<[SelectorBinding; 4]>,
    measurement_depth: usize,
    attempt: LayoutAttemptId,
    generated_by: SmallVec<[SelectorRuleId; 2]>,
}

thread_local! {
    static SELECTOR_CONTEXT: RefCell<SelectorContext> = RefCell::new(SelectorContext::default());
    static NEXT_SELECTOR_RULE_ID: Cell<u64> = const { Cell::new(1) };
    static NEXT_LAYOUT_ATTEMPT_ID: Cell<u64> = const { Cell::new(1) };
}

pub(crate) fn has_active_selectors() -> bool {
    SELECTOR_CONTEXT.with_borrow(|context| !context.bindings.is_empty())
}

// Binding depths already describe the rendered root. Shared counters reflect deferred
// execution and transaction rollback at the time of this query.
pub(crate) fn selectors_can_affect_view_contents() -> bool {
    SELECTOR_CONTEXT.with_borrow(|context| {
        let measuring = context.measurement_depth > 0;

        context.bindings.iter().any(|binding| {
            let rule = &binding.rule;

            rule.matcher.can_reach_view_contents(binding.depth)
                && rule
                    .matcher
                    .can_select_future_match(rule.next_match_idx.get(), measuring)
        })
    })
}

pub(crate) fn capture_selector_context() -> SelectorContext {
    SELECTOR_CONTEXT.with_borrow(Clone::clone)
}

pub(crate) struct SelectorContextGuard {
    previous: SelectorContext,
}

impl SelectorContextGuard {
    fn enter(context: SelectorContext) -> Self {
        let previous = SELECTOR_CONTEXT.replace(context);

        Self { previous }
    }
}

impl Drop for SelectorContextGuard {
    fn drop(&mut self) {
        SELECTOR_CONTEXT.replace(std::mem::take(&mut self.previous));
    }
}

pub(crate) fn begin_selector_layout_attempt() -> SelectorContextGuard {
    let attempt = NEXT_LAYOUT_ATTEMPT_ID.with(|next| {
        let attempt = next.get();
        next.set(
            attempt
                .checked_add(1)
                .expect("layout attempt IDs exhausted"),
        );

        LayoutAttemptId(attempt)
    });

    SelectorContextGuard::enter(SelectorContext {
        attempt,
        ..SelectorContext::default()
    })
}

pub(crate) fn with_selector_context<ResultType>(
    context: SelectorContext,
    operation: impl FnOnce() -> ResultType,
) -> ResultType {
    let context_guard = SelectorContextGuard::enter(context);
    let result = operation();
    drop(context_guard);

    result
}

pub(crate) fn generation_ancestry() -> SmallVec<[SelectorRuleId; 2]> {
    SELECTOR_CONTEXT.with_borrow(|context| context.generated_by.clone())
}

pub(crate) fn with_generation_ancestry<ResultType>(
    generated_by: SmallVec<[SelectorRuleId; 2]>,
    operation: impl FnOnce() -> ResultType,
) -> ResultType {
    let mut context = capture_selector_context();
    context.generated_by = generated_by;

    with_selector_context(context, operation)
}

fn activate_attached_selectors(selectors: &[PendingSelector]) {
    SELECTOR_CONTEXT.with_borrow_mut(|context| {
        for selector in selectors {
            if context
                .bindings
                .iter()
                .any(|binding| binding.rule.identity == selector.identity())
            {
                continue;
            }

            context.bindings.push(SelectorBinding {
                rule: selector.0.clone(),
                depth: 0,
            });
        }
    });
}

pub(crate) fn with_attached_selectors<ResultType>(
    selectors: &[PendingSelector],
    operation: impl FnOnce() -> ResultType,
) -> ResultType {
    with_selector_context(capture_selector_context(), || {
        activate_attached_selectors(selectors);

        operation()
    })
}

pub(crate) fn with_selector_measurement<ResultType>(
    operation: impl FnOnce() -> ResultType,
) -> ResultType {
    let mut context = capture_selector_context();
    context.measurement_depth += 1;

    with_selector_context(context, operation)
}

struct SelectorTransactionGuard {
    positions: SmallVec<[(Rc<SelectorRule>, usize); 4]>,
}

impl SelectorTransactionGuard {
    fn checkpoint() -> Self {
        let positions = SELECTOR_CONTEXT.with_borrow(|context| {
            context
                .bindings
                .iter()
                .filter(|binding| binding.rule.matcher.has_position())
                .map(|binding| (binding.rule.clone(), binding.rule.next_match_idx.get()))
                .collect()
        });

        Self { positions }
    }

    fn commit(&mut self) {
        self.positions.clear();
    }
}

impl Drop for SelectorTransactionGuard {
    fn drop(&mut self) {
        // Rule-owned progress survives changes to active bindings. Retained elements keep
        // their mutations and visits, so retries must rebuild children before first layout.
        for (rule, idx) in self.positions.drain(..) {
            rule.next_match_idx.set(idx);
        }
    }
}

pub(crate) fn with_selector_transaction<Success, Failure>(
    operation: impl FnOnce() -> Result<Success, Failure>,
) -> Result<Success, Failure> {
    let mut guard = SelectorTransactionGuard::checkpoint();
    let result = operation();

    if result.is_ok() {
        guard.commit();
    }

    drop(guard);

    result
}

pub(crate) fn with_deeper_selector_depth<ResultType>(
    operation: impl FnOnce() -> ResultType,
) -> ResultType {
    let mut context = capture_selector_context();

    for binding in &mut context.bindings {
        binding.depth += 1;
    }

    with_selector_context(context, operation)
}

pub(crate) fn apply_active_selectors(element: &mut AnyElement) {
    let mut selector_idx = 0;

    loop {
        let Some((binding, measuring, attempt)) = SELECTOR_CONTEXT.with_borrow(|context| {
            context
                .bindings
                .get(selector_idx)
                .cloned()
                .map(|binding| (binding, context.measurement_depth > 0, context.attempt))
        }) else {
            break;
        };

        selector_idx += 1;
        let rule = binding.rule;

        if !rule.matcher.includes_depth(binding.depth)
            || (measuring && rule.matcher.has_position())
            || element
                .selector_generation_ancestry()
                .contains(&rule.identity)
            || rule.transform.borrow().is_none()
        {
            continue;
        }

        if !element
            .selector_node_state_mut()
            .record_visit(rule.identity, attempt)
            || !rule.matcher.matches_predicates(element)
        {
            continue;
        }

        if rule.matcher.has_position() {
            let idx = rule.next_match_idx.get();
            rule.next_match_idx.set(idx + 1);

            if !rule.matcher.includes_position(idx) {
                continue;
            }
        }

        let Some(mut callback) = rule.try_lease() else {
            continue;
        };

        let attached = element.attached_selectors();
        let selected = element.take();
        let mut ancestry = selected.selector_generation_ancestry();
        ancestry.push(rule.identity);

        let replacement = with_generation_ancestry(ancestry, || {
            callback
                .transform
                .as_mut()
                .expect("selector callback lease is empty")(selected)
        });

        assert!(
            replacement.is_before_layout(),
            "selector replacement must not have requested layout"
        );

        element.replace(replacement);
        element.inherit_attached_selectors(attached);
        activate_attached_selectors(&element.attached_selectors());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnyWindowHandle, App, AppContext, AvailableSpace, Bounds, Context, Div, Element, Empty,
        Entity, FocusHandle, GlobalElementId, InspectorElementId, InteractiveElement,
        Interactivity, LayoutId, ListAlignment, ListState, ParentElement, Pixels, Render, Size,
        StatefulInteractiveElement, StyleRefinement, Styled, TestAppContext, TextStyleRefinement,
        VisualTestContext, Window, any, canvas, div, list, not, point, px, reflection::trait_set,
        rgb, rgb_to_hsla, size, uniform_list,
    };
    use std::{cell::Cell, panic, rc::Rc};

    #[gpui_macros::reflect_trait]
    trait StyledControl: crate::Styled + crate::StatefulInteractiveElement {}

    #[derive(gpui_macros::Reflect)]
    #[reflect(StyledControl)]
    struct TextOverride {
        inner: Div,
        text: TextStyleRefinement,
    }

    fn text_override(label: &'static str) -> TextOverride {
        TextOverride {
            inner: div().id(label).into_element(),
            text: TextStyleRefinement::default(),
        }
    }

    impl StyledControl for TextOverride {}

    impl Styled for TextOverride {
        fn style(&mut self) -> &mut StyleRefinement {
            self.inner.style()
        }

        fn text_style(&mut self) -> &mut TextStyleRefinement {
            &mut self.text
        }
    }

    impl InteractiveElement for TextOverride {
        fn interactivity(&mut self) -> &mut Interactivity {
            self.inner.interactivity()
        }
    }

    impl StatefulInteractiveElement for TextOverride {}

    impl ParentElement for TextOverride {
        fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
            self.inner.extend(elements);
        }
    }

    impl IntoElement for TextOverride {
        type Element = Self;

        fn into_element(self) -> Self {
            self
        }
    }

    impl Element for TextOverride {
        type RequestLayoutState = <Div as Element>::RequestLayoutState;
        type PrepaintState = <Div as Element>::PrepaintState;

        fn id(&self) -> Option<ElementId> {
            Element::id(&self.inner)
        }

        fn source_location(&self) -> Option<&'static panic::Location<'static>> {
            self.inner.source_location()
        }

        fn request_layout(
            &mut self,
            global_id: Option<&GlobalElementId>,
            inspector_id: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, Self::RequestLayoutState) {
            self.inner
                .request_layout(global_id, inspector_id, window, cx)
        }

        fn prepaint(
            &mut self,
            global_id: Option<&GlobalElementId>,
            inspector_id: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            request_layout: &mut Self::RequestLayoutState,
            window: &mut Window,
            cx: &mut App,
        ) -> Self::PrepaintState {
            self.inner
                .prepaint(global_id, inspector_id, bounds, request_layout, window, cx)
        }

        fn paint(
            &mut self,
            global_id: Option<&GlobalElementId>,
            inspector_id: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            request_layout: &mut Self::RequestLayoutState,
            prepaint: &mut Self::PrepaintState,
            window: &mut Window,
            cx: &mut App,
        ) {
            self.inner.paint(
                global_id,
                inspector_id,
                bounds,
                request_layout,
                prepaint,
                window,
                cx,
            );
        }
    }

    struct SelectorTestView {
        render: Box<dyn Fn() -> AnyElement>,
    }

    impl Render for SelectorTestView {
        #[allow(unused_variables)]
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            (self.render)()
        }
    }

    fn draw_selector_window(window: AnyWindowHandle, cx: &mut TestAppContext) {
        cx.update_window(window, |_view, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }

    fn record_match(matches: &Rc<RefCell<Vec<ElementId>>>, element: &AnyElement) {
        let mut matches = matches.borrow_mut();
        assert!(matches.len() < 16, "selector rewrite did not terminate");
        matches.push(element.element_id().unwrap());
    }

    fn draw_selector_tree(
        cx: &mut TestAppContext,
        build: impl FnOnce() -> AnyElement,
    ) -> &mut VisualTestContext {
        let visual = cx.add_empty_window();
        visual.draw(
            Default::default(),
            size(px(100.), px(100.)),
            |_window, _cx| build(),
        );

        visual
    }

    #[track_caller]
    fn assert_matches(matches: &Rc<RefCell<Vec<ElementId>>>, expected: &[&'static str]) {
        let expected = expected
            .iter()
            .map(|label| ElementId::from(*label))
            .collect::<Vec<_>>();
        assert_eq!(*matches.borrow(), expected);
    }

    fn selector_row(label: &'static str) -> Div {
        div()
            .id(label)
            .debug_selector(move || label.into())
            .size(px(8.))
            .flex_shrink_0()
            .into_element()
    }

    fn layout_selector_row(label: &'static str, window: &mut Window, cx: &mut App) {
        let mut element = selector_row(label).class("row");
        element.layout_as_root(AvailableSpace::min_size(), window, cx);
    }

    fn deferred_selector_row(label: &'static str) -> AnyElement {
        crate::deferred(crate::container_query(move |_size, _window, _cx| {
            selector_row(label).class("row")
        }))
        .into_any_element()
    }

    fn layout_nested_transaction_row(
        fails: bool,
        attached: Rc<RefCell<Vec<ElementId>>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), ()> {
        with_selector_context(capture_selector_context(), || {
            window.transact(|window| {
                SELECTOR_CONTEXT.with_borrow_mut(|context| context.bindings.reverse());

                let mut element = selector_row("inner").class("row").select(
                    Select::this().class("row").every(1),
                    move |element| {
                        record_match(&attached, &element.element);

                        // Checkpoints can retain a rule while its callback is leased.
                        assert!(with_selector_transaction(|| Err::<(), ()>(())).is_err());

                        element
                    },
                );
                element.layout_as_root(AvailableSpace::min_size(), window, cx);

                // Removed bindings still share progress with the enclosing scope.
                SELECTOR_CONTEXT.with_borrow_mut(|context| context.bindings.clear());

                if fails {
                    return Err(());
                }

                Ok(())
            })
        })
    }

    fn list_retry_row(idx: usize, focus_handle: &FocusHandle) -> AnyElement {
        let label = ["row-0", "row-1", "row-2", "row-3", "row-4", "row-5"][idx];
        let mut row = selector_row(label).w_full();

        if idx == 2 {
            row = row.child(
                canvas(
                    |bounds, window, _cx| {
                        window.request_autoscroll(Bounds::from_corners(
                            point(bounds.left(), bounds.top() - px(36.)),
                            point(bounds.right(), bounds.top() + px(5.)),
                        ));
                    },
                    |_bounds, _state, _window, _cx| {},
                )
                .size_full(),
            );
        }

        if idx == 5 {
            return row.track_focus(focus_handle).class("row");
        }

        row.class("row")
    }

    #[crate::test]
    fn wrapping_preserves_identity_metadata_and_original_descendants(cx: &mut TestAppContext) {
        let wrapped = Rc::new(RefCell::new(Vec::new()));
        let wrapper_children = Rc::new(RefCell::new(Vec::new()));
        let styled = Rc::new(RefCell::new(Vec::new()));

        let matched = wrapped.clone();
        let children = wrapper_children.clone();
        let renamed = styled.clone();
        let attached = styled.clone();

        let visual = draw_selector_tree(cx, move || {
            div()
                .child(
                    div()
                        .id("outer")
                        .debug_selector(|| "outer".into())
                        .child(
                            div()
                                .id("inner")
                                .debug_selector(|| "inner".into())
                                .class(["icon", "leaf"]),
                        )
                        .class("icon")
                        .select(
                            Select::descendants().class("leaf").reflects(crate::Styled),
                            move |element| {
                                record_match(&attached, &element.element);

                                element.size(px(12.))
                            },
                        ),
                )
                .select(
                    Select::descendants()
                        .id("outer")
                        .reflects(crate::InteractiveElement),
                    |element| element.id("renamed"),
                )
                .select(
                    Select::descendants()
                        .id("renamed")
                        .class("icon")
                        .reflects(crate::Styled),
                    move |element| {
                        record_match(&renamed, &element.element);

                        element.size(px(48.))
                    },
                )
                .select(Select::descendants().class("icon"), move |element| {
                    record_match(&matched, &element.element);
                    let children = children.clone();

                    div()
                        .child(element)
                        .select(Select::children(), move |element| {
                            record_match(&children, &element.element);

                            element
                        })
                })
        });

        assert_matches(&wrapped, &["renamed", "inner"]);
        assert_eq!(*wrapper_children.borrow(), *wrapped.borrow());
        assert_matches(&styled, &["renamed", "inner"]);
        assert_eq!(
            visual.debug_bounds("outer").unwrap().size,
            size(px(48.), px(48.))
        );
        assert_eq!(
            visual.debug_bounds("inner").unwrap().size,
            size(px(12.), px(12.))
        );
    }

    #[crate::test]
    fn dispatch_and_generation_survive_mutation_and_replacement(cx: &mut TestAppContext) {
        let initial_color = rgb_to_hsla(rgb(0x123456));
        let later_color = rgb_to_hsla(rgb(0x654321));

        let generating = Rc::new(RefCell::new(Vec::new()));
        let reflected = Rc::new(RefCell::new(Vec::new()));
        let later = Rc::new(RefCell::new(Vec::new()));
        let installed = Rc::new(RefCell::new(Vec::new()));

        let first = generating.clone();
        let dispatch = reflected.clone();
        let second = later.clone();
        let new_rules = installed.clone();

        let visual = draw_selector_tree(cx, move || {
            let mut prebuilt = Some(div().id("prebuilt").class("icon"));
            let mut concrete = Some(div().id("captured-concrete"));

            div()
                .child(text_override("mutated").class("icon"))
                .child(div().id("replaced").class("icon"))
                .select(
                    Select::descendants().class("icon").reflects(trait_set!(
                        crate::Styled,
                        crate::InteractiveElement,
                        crate::ParentElement
                    )),
                    move |element| {
                        record_match(&first, &element.element);

                        let mut element = element.text_color(initial_color);
                        let label = element.element.element_id().unwrap();
                        let expected_base_color = if label == ElementId::from("mutated") {
                            None
                        } else {
                            Some(initial_color)
                        };

                        assert_eq!(element.text_style().color, Some(initial_color));
                        assert_eq!(element.style().text.color, expected_base_color);

                        if label == ElementId::from("replaced") {
                            return text_override("from-first")
                                .debug_selector(|| "from-first".into())
                                .text_color(initial_color)
                                .child(div().id("fresh-child").class("icon"))
                                .class("icon");
                        }

                        if label == ElementId::from("mutated") {
                            return element
                                .id("renamed")
                                .child(div().id("appended").class("icon"))
                                .child(prebuilt.take().unwrap())
                                .child(concrete.take().unwrap().class("icon"))
                                .into_any_element();
                        }

                        element.into_any_element()
                    },
                )
                .select(
                    Select::descendants().class("icon").reflects(StyledControl),
                    move |mut element| {
                        record_match(&dispatch, &element.element);
                        let label = element.element.element_id();

                        assert_eq!(element.text_style().color, Some(initial_color));
                        assert!(element.style().text.color.is_none());
                        assert_eq!(element.interactivity().element_id, label);

                        let mut element = element.text_color(later_color);
                        let concrete = element.element.downcast_mut::<TextOverride>().unwrap();

                        assert_eq!(concrete.text.color, Some(later_color));

                        element.id(label.unwrap())
                    },
                )
                .select(Select::descendants().class("icon"), move |element| {
                    record_match(&second, &element.element);

                    if element.element.element_id() != Some(ElementId::from("from-first")) {
                        return element.into_any_element();
                    }

                    let this = new_rules.clone();
                    let children = new_rules.clone();

                    div()
                        .id("from-second")
                        .debug_selector(|| "from-second".into())
                        .child(element)
                        .child(
                            div()
                                .id("generated-child")
                                .debug_selector(|| "generated-child".into())
                                .class("icon"),
                        )
                        .class("icon")
                        .select(Select::this().reflects(crate::Styled), move |element| {
                            record_match(&this, &element.element);

                            element.size(px(40.))
                        })
                        .select(Select::children().reflects(crate::Styled), move |element| {
                            record_match(&children, &element.element);

                            element.size(px(8.))
                        })
                })
        });

        assert_matches(&generating, &["mutated", "prebuilt", "replaced"]);
        assert_matches(&reflected, &["renamed", "from-first"]);
        assert_matches(
            &later,
            &[
                "renamed",
                "appended",
                "prebuilt",
                "captured-concrete",
                "from-first",
                "fresh-child",
            ],
        );
        assert_matches(
            &installed,
            &["from-second", "from-first", "generated-child"],
        );
        assert_eq!(
            visual.debug_bounds("from-second").unwrap().size,
            size(px(40.), px(40.))
        );

        for label in ["from-first", "generated-child"] {
            assert_eq!(
                visual.debug_bounds(label).unwrap().size,
                size(px(8.), px(8.))
            );
        }
    }

    #[crate::test]
    fn generated_factories_preserve_origins_and_original_deferred_subtrees(
        cx: &mut TestAppContext,
    ) {
        let generating = Rc::new(RefCell::new(Vec::new()));
        let later = Rc::new(RefCell::new(Vec::new()));

        let generating_for_render = generating.clone();
        let later_for_render = later.clone();
        let window = cx.add_window(move |_window, _cx| SelectorTestView {
            render: Box::new(move || {
                let matches = generating_for_render.clone();
                let concrete = generating_for_render.clone();
                let later = later_for_render.clone();

                div()
                    .size_full()
                    .child(
                        div()
                            .id("original")
                            .child(div().id("original-child").class("icon"))
                            .child(crate::container_query(|_size, _window, _cx| {
                                div().id("original-late").class("icon")
                            }))
                            .class("icon"),
                    )
                    .child(crate::deferred(crate::container_query(
                        |_size, _window, _cx| div().id("ordinary-late").class("icon"),
                    )))
                    .child(div().id("concrete-original").class("factory"))
                    .select(Select::descendants().class("icon"), move |element| {
                        record_match(&matches, &element.element);

                        if element.element.element_id() != Some(ElementId::from("original")) {
                            return element.into_any_element();
                        }

                        crate::deferred(crate::container_query(move |_size, _window, _cx| {
                            div()
                                .id("generated-late")
                                .child(element)
                                .child(crate::deferred(crate::container_query(
                                    |_size, _window, _cx| {
                                        div().id("nested-generated").class("icon")
                                    },
                                )))
                                .class("icon")
                        }))
                        .into_any_element()
                    })
                    .select(Select::descendants().class("factory"), move |element| {
                        record_match(&concrete, &element.element);

                        crate::container_query(|_size, _window, _cx| {
                            div().id("concrete-generated").class("factory")
                        })
                    })
                    .select(
                        Select::descendants()
                            .class(any(["icon", "factory"]))
                            .reflects(crate::Styled),
                        move |element| {
                            record_match(&later, &element.element);

                            element.size(px(16.))
                        },
                    )
            }),
        });

        generating.borrow_mut().clear();
        later.borrow_mut().clear();
        draw_selector_window(window.into(), cx);

        assert_matches(
            &generating,
            &[
                "original",
                "concrete-original",
                "original-child",
                "original-late",
                "ordinary-late",
            ],
        );
        assert_matches(
            &later,
            &[
                "concrete-generated",
                "generated-late",
                "original",
                "original-child",
                "original-late",
                "ordinary-late",
                "nested-generated",
            ],
        );
    }

    #[crate::test]
    fn nested_deferred_scopes_share_positions_and_callback_state(cx: &mut TestAppContext) {
        let visited = Rc::new(RefCell::new(Vec::new()));
        let direct = Rc::new(RefCell::new(Vec::new()));
        let indexed = Rc::new(RefCell::new(Vec::new()));

        let visited_for_render = visited.clone();
        let direct_for_render = direct.clone();
        let indexed_for_render = indexed.clone();
        let window = cx.add_window(move |_window, _cx| SelectorTestView {
            render: Box::new(move || {
                let visited = visited_for_render.clone();
                let direct = direct_for_render.clone();
                let late_direct = direct_for_render.clone();
                let nested_direct = direct_for_render.clone();
                let indexed = indexed_for_render.clone();
                let mut match_count = 0;

                let nested = crate::deferred(
                    crate::container_query(|_size, _window, _cx| {
                        selector_row("nested-first")
                            .child(selector_row("nested-second").class("row"))
                            .class("row")
                    })
                    .select(
                        Select::children().class("row").reflects(crate::Styled),
                        move |element| {
                            record_match(&nested_direct, &element.element);

                            element.h(px(16.))
                        },
                    ),
                )
                .priority(0);
                let low_priority = crate::deferred(
                    crate::container_query(move |_size, _window, _cx| {
                        selector_row("late-low").child(nested).class("row")
                    })
                    .select(
                        Select::children().class("row").reflects(crate::Styled),
                        move |element| {
                            record_match(&late_direct, &element.element);

                            element.h(px(12.))
                        },
                    ),
                )
                .priority(10);

                div()
                    .size_full()
                    .child(selector_row("synchronous").class("row"))
                    .child(
                        crate::deferred(crate::container_query(|_size, _window, _cx| {
                            selector_row("late-high").class("row")
                        }))
                        .priority(20),
                    )
                    .child(low_priority)
                    .select(
                        Select::children().class("row").reflects(crate::Styled),
                        move |element| {
                            record_match(&direct, &element.element);

                            element.h(px(10.))
                        },
                    )
                    .select(Select::descendants().class("row"), move |element| {
                        record_match(&visited, &element.element);

                        element
                    })
                    .select(
                        Select::descendants()
                            .class("row")
                            .reflects(crate::Styled)
                            .every(2),
                        move |element| {
                            record_match(&indexed, &element.element);
                            match_count += 1;

                            element.w(px(10. + 10. * match_count as f32))
                        },
                    )
            }),
        });

        for _redraw in 0..2 {
            visited.borrow_mut().clear();
            direct.borrow_mut().clear();
            indexed.borrow_mut().clear();
            draw_selector_window(window.into(), cx);

            assert_matches(
                &visited,
                &[
                    "synchronous",
                    "late-low",
                    "late-high",
                    "nested-first",
                    "nested-second",
                ],
            );
            assert_matches(&direct, &["synchronous", "late-low", "nested-first"]);
            assert_matches(&indexed, &["synchronous", "late-high", "nested-second"]);

            cx.update_window(window.into(), |_view, window, _cx| {
                for (label, width, height) in [
                    ("synchronous", 20., 10.),
                    ("late-low", 8., 12.),
                    ("late-high", 30., 8.),
                    ("nested-first", 8., 16.),
                    ("nested-second", 40., 8.),
                ] {
                    assert_eq!(
                        window.rendered_frame.debug_bounds[label].size,
                        size(px(width), px(height))
                    );
                }
            })
            .unwrap();
        }
    }

    #[crate::test]
    fn positions_ignore_generated_nodes_and_repeated_or_abandoned_layout(cx: &mut TestAppContext) {
        for select_nth in [false, true] {
            let failed = Rc::new(Cell::new(0));
            let indexed = Rc::new(RefCell::new(Vec::new()));

            let failed_count = failed.clone();
            let matched = indexed.clone();

            let selector = Select::descendants().class("row");
            let selector = if select_nth {
                selector.nth(1)
            } else {
                selector.every(2)
            };

            let retained_size = if select_nth { px(5.) } else { px(20.) };

            draw_selector_tree(cx, move || {
                div()
                    .child(crate::container_query(move |_size, window, cx| {
                        let mut retained = div().id(("row", 0_usize)).size(px(5.)).class("row");
                        let available = AvailableSpace::min_size();
                        let expected_size = size(retained_size, retained_size);
                        let result = window.transact(|window| {
                            assert_eq!(
                                retained.layout_as_root(available, window, cx),
                                expected_size
                            );

                            Err::<(), ()>(())
                        });

                        assert!(result.is_err());
                        for available in [
                            available,
                            size(
                                AvailableSpace::Definite(px(50.)),
                                AvailableSpace::MinContent,
                            ),
                        ] {
                            assert_eq!(
                                retained.layout_as_root(available, window, cx),
                                expected_size
                            );
                        }

                        // A new attempt cannot make a requested drawable transform again.
                        with_selector_context(
                            SelectorContext {
                                attempt: LayoutAttemptId(u64::MAX),
                                ..capture_selector_context()
                            },
                            || {
                                assert_eq!(
                                    retained.layout_as_root(available, window, cx),
                                    expected_size
                                );
                            },
                        );

                        div().children((1_usize..5).map(|idx| div().id(("row", idx)).class("row")))
                    }))
                    .select(Select::descendants().class("renamed"), move |element| {
                        failed_count.set(failed_count.get() + 1);

                        element
                    })
                    .select(selector, move |element| {
                        record_match(&matched, &element.element);

                        div()
                            .size(px(20.))
                            .child(element.class("renamed"))
                            .class("row")
                    })
            });

            let expected = if select_nth {
                vec![2_usize]
            } else {
                vec![0_usize, 1, 3]
            };

            let expected = expected
                .into_iter()
                .map(|idx| ElementId::from(("row", idx)))
                .collect::<Vec<_>>();
            assert_eq!(failed.get(), 0);
            assert_eq!(*indexed.borrow(), expected);
        }
    }

    #[crate::test]
    fn caught_transform_panics_restore_callbacks_and_construction_context(cx: &mut TestAppContext) {
        let selected = Rc::new(RefCell::new(Vec::new()));
        let observed = selected.clone();
        let recovered = selected.clone();

        draw_selector_tree(cx, move || {
            div()
                .child(crate::container_query(move |_size, window, cx| {
                    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        window.with_layout_measurement(|window| {
                            let mut doomed = div().id("panic").class("row");
                            doomed.layout_as_root(AvailableSpace::min_size(), window, cx);
                        });
                    }));

                    assert!(result.is_err());

                    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        let _result = window.transact(|window| {
                            layout_selector_row("transaction-panic", window, cx);

                            Ok::<(), ()>(())
                        });
                    }));

                    assert!(result.is_err());

                    div().id("surviving").class("row")
                }))
                .select(Select::descendants().class("row"), move |element| {
                    if element.element.element_id() == Some(ElementId::from("panic")) {
                        let _generated = div().class("row");
                        panic!("transform failed");
                    }

                    record_match(&recovered, &element.element);

                    element
                })
                .select(Select::descendants().class("row").nth(0), move |element| {
                    record_match(&observed, &element.element);

                    if element.element.element_id() == Some(ElementId::from("transaction-panic")) {
                        let _generated = div().class("row");
                        panic!("positional transform failed");
                    }

                    element
                })
        });

        assert_matches(
            &selected,
            &[
                "transaction-panic",
                "transaction-panic",
                "surviving",
                "surviving",
            ],
        );
        assert!(!has_active_selectors());
        assert!(generation_ancestry().is_empty());

        draw_selector_tree(cx, || {
            div()
                .child(div().class("row"))
                .select(Select::descendants().class("row"), |element| element)
        });
    }

    #[crate::test]
    #[allow(unused_variables)]
    fn combines_class_conditions_with_scalar_ids(cx: &mut TestAppContext) {
        let selected = Rc::new(RefCell::new(Vec::new()));
        let selected_for_render = selected.clone();
        let window = cx.add_window(move |window, cx| SelectorTestView {
            render: Box::new(move || {
                let selected = selected_for_render.clone();

                div()
                    .child(div().id(("row", 0_usize)).class(["apple", "pear"]))
                    .child(div().id(("row", 1_usize)).class(("apple", "pear", "plum")))
                    .child(div().id(("row", 2_usize)).class(vec!["apple", "pear"]))
                    .child(div().id(("row", 3_usize)).class(&["pear"]))
                    .child(div().class("apple"))
                    .select(
                        Select::children()
                            .reflects(crate::Styled)
                            .class((any(["apple", "orange"]), not("plum")))
                            .class("pear")
                            .id(not(("row", 0_usize))),
                        move |element| {
                            selected.borrow_mut().push(element.element.element_id());

                            element.bg(rgb(0x112233))
                        },
                    )
            }),
        });

        selected.borrow_mut().clear();
        draw_selector_window(window.into(), cx);

        assert_eq!(
            *selected.borrow(),
            vec![Some(ElementId::from(("row", 2_usize)))]
        );
    }

    #[crate::test]
    #[allow(unused_variables)]
    fn indexes_filtered_elements_across_layout_and_prepaint(cx: &mut TestAppContext) {
        let selected = Rc::new(RefCell::new(Vec::new()));
        let selected_for_render = selected.clone();
        let window = cx.add_window(move |window, cx| SelectorTestView {
            render: Box::new(move || {
                let selectors = [
                    ("nth", Select::descendants().class("row").nth(2)),
                    ("reordered", Select::descendants().nth(2).class("row")),
                    ("every", Select::descendants().class("row").every(2)),
                    ("both", Select::descendants().class("row").nth(2).every(2)),
                    (
                        "disjoint",
                        Select::descendants().class("row").nth(1).every(2),
                    ),
                    ("missing", Select::descendants().class("row").nth(99)),
                ];
                let mut root = div()
                    .child(div().id("first").class("row"))
                    .child(div().id("unrelated"))
                    .child(div().id("second").class("row"))
                    .child(crate::container_query(|size, window, cx| {
                        div()
                            .id("late")
                            .child(div().id("late-child").class("row"))
                            .class("row")
                    }))
                    .into_any_element();

                for (label, selector) in selectors {
                    let selected = selected_for_render.clone();
                    root = root.select(selector, move |element| {
                        selected
                            .borrow_mut()
                            .push((label, element.element.element_id().unwrap()));

                        element
                    });
                }

                root
            }),
        });
        let expected = vec![
            ("every", ElementId::from("first")),
            ("nth", ElementId::from("late")),
            ("reordered", ElementId::from("late")),
            ("every", ElementId::from("late")),
            ("both", ElementId::from("late")),
        ];

        for redraw in 0..2 {
            selected.borrow_mut().clear();
            draw_selector_window(window.into(), cx);

            assert_eq!(*selected.borrow(), expected);
        }
    }

    #[crate::test]
    #[allow(unused_variables)]
    fn positional_selectors_exclude_list_measurements(cx: &mut TestAppContext) {
        let selected = Rc::new(RefCell::new(Vec::new()));
        let first = Rc::new(RefCell::new(Vec::new()));
        let selected_for_render = selected.clone();
        let first_for_render = first.clone();
        let state = ListState::new(8, ListAlignment::Top, px(0.)).measure_all();
        let window = cx.add_window(move |window, cx| SelectorTestView {
            render: Box::new(move || {
                let selected = selected_for_render.clone();
                let first = first_for_render.clone();

                div()
                    .flex()
                    .flex_col()
                    .w(px(80.))
                    .h(px(120.))
                    .child(
                        uniform_list("uniform", 8, |range, window, cx| {
                            range
                                .map(|idx| {
                                    div()
                                        .id(("uniform-row", idx))
                                        .h(px(8.))
                                        .class("row")
                                        .into_any_element()
                                })
                                .collect()
                        })
                        .h(px(60.))
                        .w_full(),
                    )
                    .child(
                        list(state.clone(), |idx, window, cx| {
                            div().id(("list-row", idx)).h(px(8.)).class("row")
                        })
                        .h(px(60.))
                        .w_full(),
                    )
                    .select(
                        Select::descendants().class("row").reflects(crate::Styled),
                        |element| element.h(px(20.)),
                    )
                    .select(
                        Select::descendants().class("row").every(1),
                        move |element| {
                            selected
                                .borrow_mut()
                                .push(element.element.element_id().unwrap());

                            element
                        },
                    )
                    .select(Select::descendants().class("row").nth(0), move |element| {
                        first
                            .borrow_mut()
                            .push(element.element.element_id().unwrap());

                        element
                    })
            }),
        });
        let expected = ["uniform-row", "list-row"]
            .into_iter()
            .flat_map(|label| (0_usize..3).map(move |idx| ElementId::from((label, idx))))
            .collect::<Vec<_>>();

        for redraw in 0..2 {
            selected.borrow_mut().clear();
            first.borrow_mut().clear();
            draw_selector_window(window.into(), cx);

            assert_eq!(*selected.borrow(), expected);
            assert_eq!(
                *first.borrow(),
                vec![ElementId::from(("uniform-row", 0_usize))]
            );
        }
    }

    #[crate::test]
    fn list_autoscroll_retries_rebuild_visible_and_focused_rows(cx: &mut TestAppContext) {
        for focus_offscreen in [false, true] {
            let state = ListState::new(6, ListAlignment::Top, px(0.)).measure_all();
            let attempted = Rc::new(RefCell::new(Vec::new()));
            let first = Rc::new(RefCell::new(Vec::new()));
            let every = Rc::new(RefCell::new(Vec::new()));

            let attempted_matches = attempted.clone();
            let first_matches = first.clone();
            let every_matches = every.clone();
            let state_for_render = state.clone();

            let window = cx.add_window(move |window, cx| {
                let focus_handle = cx.focus_handle();
                state_for_render.splice_focusable(
                    0..6,
                    (0..6).map(|idx| (idx == 5).then(|| focus_handle.clone())),
                );

                if focus_offscreen {
                    window.focus(&focus_handle, cx);
                }

                SelectorTestView {
                    render: Box::new(move || {
                        let focus_handle = focus_handle.clone();
                        let attempted_matches = attempted_matches.clone();
                        let first_matches = first_matches.clone();
                        let every_matches = every_matches.clone();

                        div()
                            .w(px(80.))
                            .h(px(60.))
                            .child(
                                list(state_for_render.clone(), move |idx, _window, _cx| {
                                    list_retry_row(idx, &focus_handle)
                                })
                                .size_full(),
                            )
                            .select(
                                Select::descendants().class("row").reflects(crate::Styled),
                                |element| element.h(px(24.)),
                            )
                            .select(
                                Select::descendants().class("row").every(1),
                                move |element| {
                                    record_match(&attempted_matches, &element.element);

                                    element
                                },
                            )
                            .select(
                                Select::descendants()
                                    .class("row")
                                    .reflects(crate::Styled)
                                    .nth(0),
                                move |element| {
                                    record_match(&first_matches, &element.element);

                                    element.w(px(48.))
                                },
                            )
                            .select(
                                Select::descendants().class("row").every(2),
                                move |element| {
                                    record_match(&every_matches, &element.element);

                                    element
                                },
                            )
                    }),
                }
            });

            attempted.borrow_mut().clear();
            first.borrow_mut().clear();
            every.borrow_mut().clear();
            state.scroll_to(crate::ListOffset {
                item_ix: 2,
                offset_in_item: px(0.),
            });
            draw_selector_window(window.into(), cx);

            assert_eq!(state.logical_scroll_top().item_ix, 0);
            assert_eq!(state.logical_scroll_top().offset_in_item, px(12.));
            assert_eq!(state.max_offset_for_scrollbar().y, px(84.));
            assert_matches(&first, &["row-2", "row-0"]);
            assert_matches(&every, &["row-2", "row-4", "row-0", "row-2"]);

            let expected = if focus_offscreen {
                vec![
                    "row-2", "row-3", "row-4", "row-5", "row-0", "row-1", "row-2", "row-5",
                ]
            } else {
                vec!["row-2", "row-3", "row-4", "row-0", "row-1", "row-2"]
            };
            assert_matches(&attempted, &expected);

            cx.update_window(window.into(), |_view, window, _cx| {
                for (label, top, width) in [
                    ("row-0", -12., 48.),
                    ("row-1", 12., 80.),
                    ("row-2", 36., 80.),
                ] {
                    let bounds = window.rendered_frame.debug_bounds[label];
                    assert_eq!(bounds.origin.y, px(top));
                    assert_eq!(bounds.size, size(px(width), px(24.)));
                }
            })
            .unwrap();
        }
    }

    #[crate::test]
    fn measurement_helpers_preserve_selector_state(cx: &mut TestAppContext) {
        let available_space = size(AvailableSpace::MaxContent, AvailableSpace::MinContent);
        let expected_size = size(px(48.), px(24.));

        let measure_nested_row = move |window: &mut Window, cx: &mut App| {
            let mut element = div().id("nested-measurement").size(px(8.)).class("row");

            element.layout_as_root(available_space, window, cx)
        };

        let build_measured_row = move |window: &mut Window, cx: &mut App| {
            let nested_size =
                window.with_layout_measurement(|window| measure_nested_row(window, cx));
            let mut element = div().id("during-construction").size(px(8.)).class("row");
            let constructed_size = element.layout_as_root(available_space, window, cx);

            assert_eq!([nested_size, constructed_size], [expected_size; 2]);

            div().id("measured-root").size(px(8.)).class("row")
        };

        let build_panicking_row = |_window: &mut Window, _cx: &mut App| -> Empty {
            panic!("measurement construction failed");
        };

        let render_rows = move |_bounds: Size<Pixels>, window: &mut Window, cx: &mut App| {
            let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                window.measure_element(available_space, cx, build_panicking_row);
            }));

            assert!(result.is_err());

            let measured_size = window.measure_element(available_space, cx, build_measured_row);

            assert_eq!(measured_size, expected_size);

            div()
                .child(div().id("first").class("row"))
                .child(div().id("unrelated"))
                .child(div().id("second").class("row"))
        };

        let selected = Rc::new(RefCell::new(Vec::new()));
        let first = selected.clone();
        let every = selected.clone();

        cx.add_empty_window().draw(
            Default::default(),
            size(px(80.), px(120.)),
            move |_window, _cx| {
                div()
                    .child(crate::container_query(render_rows))
                    .select(
                        Select::descendants().class("row").reflects(crate::Styled),
                        |element| element.w(px(48.)).h(px(24.)),
                    )
                    .select(
                        Select::descendants()
                            .class("row")
                            .reflects(crate::Styled)
                            .nth(0),
                        move |element| {
                            first
                                .borrow_mut()
                                .push(("nth", element.element.element_id().unwrap()));

                            element.h(px(99.))
                        },
                    )
                    .select(
                        Select::descendants().class("row").every(2),
                        move |element| {
                            every
                                .borrow_mut()
                                .push(("every", element.element.element_id().unwrap()));

                            element
                        },
                    )
            },
        );

        assert_eq!(
            *selected.borrow(),
            vec![
                ("nth", ElementId::from("first")),
                ("every", ElementId::from("first")),
            ]
        );
    }

    struct CachedSelectorChild {
        renders: Rc<Cell<usize>>,
    }

    impl Render for CachedSelectorChild {
        #[allow(unused_variables)]
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);

            div()
                .id("cached-row")
                .size_full()
                .occlude()
                .bg(rgb(0x112233))
                .debug_selector(|| "cached-row".into())
                .class("row")
        }
    }

    fn cached_selector_style() -> StyleRefinement {
        StyleRefinement::default().w(px(40.)).h(px(20.))
    }

    fn cached_selector_element(child: &Entity<CachedSelectorChild>) -> AnyElement {
        child
            .clone()
            .cached(cached_selector_style())
            .into_any_element()
    }

    fn cached_selector_window(
        cx: &mut TestAppContext,
        render: impl Fn(&Entity<CachedSelectorChild>, &Rc<Cell<usize>>) -> AnyElement + 'static,
    ) -> (
        AnyWindowHandle,
        Entity<CachedSelectorChild>,
        Rc<Cell<usize>>,
    ) {
        let renders = Rc::new(Cell::new(0));
        let child = cx.new(|_cx| CachedSelectorChild {
            renders: renders.clone(),
        });

        let child_for_render = child.clone();
        let renders_for_render = renders.clone();

        let window = cx
            .add_window(move |_window, _cx| SelectorTestView {
                render: Box::new(move || render(&child_for_render, &renders_for_render)),
            })
            .into();

        draw_selector_window(window, cx);
        renders.set(0);

        (window, child, renders)
    }

    fn measure_cached_selector_child(
        child: &Entity<CachedSelectorChild>,
        non_positional: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        window
            .transact(|window| {
                let measured_size = window.with_layout_measurement(|window| {
                    let mut measured = cached_selector_element(child);
                    let measured_size =
                        measured.layout_as_root(AvailableSpace::min_size(), window, cx);

                    if non_positional {
                        measured.prepaint(window, cx);
                        assert_eq!(
                            window.next_frame.hitboxes.last().unwrap().bounds.size,
                            size(px(40.), px(12.))
                        );
                    }

                    measured_size
                });

                Err::<(), Size<Pixels>>(measured_size)
            })
            .unwrap_err()
    }

    #[track_caller]
    fn assert_cached_selector_size(
        window: AnyWindowHandle,
        expected: Size<Pixels>,
        cx: &mut TestAppContext,
    ) {
        cx.update_window(window, |_view, window, _cx| {
            let hitbox = window.rendered_frame.hitboxes.last().unwrap();
            assert_eq!(hitbox.bounds.size, expected);
            assert!(!window.rendered_frame.scene.quads.is_empty());
        })
        .unwrap();
    }

    #[crate::test]
    fn cached_selectors_respect_scope_and_view_depth(cx: &mut TestAppContext) {
        for (scope, on_view) in [
            (SelectScope::This, false),
            (SelectScope::Children, false),
            (SelectScope::Children, true),
        ] {
            let selected = Rc::new(Cell::new(0));
            let observed = selected.clone();
            let (window, _child, renders) = cached_selector_window(cx, move |child, _renders| {
                let selected = observed.clone();
                let cached = cached_selector_element(child);

                if on_view {
                    return div()
                        .child(cached.select(
                            Select::children().reflects(crate::Styled),
                            move |element| {
                                selected.set(selected.get() + 1);

                                element.h(px(12.))
                            },
                        ))
                        .into_any_element();
                }

                div()
                    .child(cached)
                    .select(Select::new(scope), move |element| {
                        selected.set(selected.get() + 1);

                        element
                    })
            });

            selected.set(0);

            for _redraw in 0..2 {
                draw_selector_window(window, cx);
                let height = if on_view { px(12.) } else { px(20.) };
                assert_cached_selector_size(window, size(px(40.), height), cx);
            }

            assert_eq!(renders.get(), if on_view { 2 } else { 0 });
            assert_eq!(selected.get(), 2);
        }
    }

    #[crate::test]
    fn cached_selectors_use_live_positional_exhaustion(cx: &mut TestAppContext) {
        for (nth, every, earlier_match, deferred_contents, expected_renders, expected_height) in [
            (Some(0), None, true, false, 0, 20.),
            (Some(0), Some(2), true, false, 0, 20.),
            (Some(0), None, false, false, 2, 12.),
            (Some(1), None, true, false, 2, 12.),
            (None, Some(2), true, false, 2, 20.),
            (Some(0), None, true, true, 0, 20.),
        ] {
            let selected = Rc::new(RefCell::new(Vec::new()));
            let observed = selected.clone();
            let (window, _child, renders) = cached_selector_window(cx, move |child, _renders| {
                let selected = observed.clone();
                let mut selector = Select::descendants().class("row").reflects(crate::Styled);

                if let Some(idx) = nth {
                    selector = selector.nth(idx);
                }

                if let Some(step) = every {
                    selector = selector.every(step);
                }

                let cached = cached_selector_element(child);
                let first = selector_row("first").class("row");
                let children = match (earlier_match, deferred_contents) {
                    (true, true) => vec![
                        crate::deferred(crate::container_query(move |_size, _window, _cx| cached))
                            .priority(2)
                            .into_any_element(),
                        crate::deferred(crate::container_query(move |_size, _window, _cx| first))
                            .priority(1)
                            .into_any_element(),
                    ],
                    (true, false) => vec![first, cached],
                    (false, _) => vec![cached],
                };

                div()
                    .size(px(80.))
                    .children(children)
                    .select(selector, move |element| {
                        record_match(&selected, &element.element);

                        element.h(px(12.))
                    })
            });

            selected.borrow_mut().clear();

            for _redraw in 0..2 {
                draw_selector_window(window, cx);
                assert_cached_selector_size(window, size(px(40.), px(expected_height)), cx);
            }

            let target = if earlier_match && nth != Some(1) {
                "first"
            } else {
                "cached-row"
            };

            assert_matches(&selected, &[target, target]);
            assert_eq!(renders.get(), expected_renders);
        }
    }

    #[crate::test]
    fn cached_selectors_recheck_progress_after_measurements_and_rollbacks(cx: &mut TestAppContext) {
        for non_positional in [false, true] {
            let selected = Rc::new(RefCell::new(Vec::new()));
            let observed = selected.clone();
            let (window, _child, renders) = cached_selector_window(cx, move |child, renders| {
                let child = child.clone();
                let renders = renders.clone();
                let selected = observed.clone();
                let mut root = div()
                    .size(px(80.))
                    .child(crate::container_query(move |_size, window, cx| {
                        let available = AvailableSpace::min_size();
                        let before = renders.get();
                        let measured_size = window.with_layout_measurement(|window| {
                            measure_cached_selector_child(&child, non_positional, window, cx)
                        });
                        assert_eq!(measured_size, size(px(40.), px(20.)));
                        // Temporary cached views still render to probe their text direction.
                        assert_eq!(renders.get() - before, 1);

                        let outer = window.transact(|window| {
                            let inner = window.transact(|window| {
                                layout_selector_row("abandoned-first", window, cx);
                                let before = renders.get();
                                cached_selector_element(&child)
                                    .layout_as_root(available, window, cx);
                                assert_eq!(renders.get() - before, 1);

                                Err::<(), ()>(())
                            });

                            assert!(inner.is_err());
                            cached_selector_element(&child).layout_as_root(available, window, cx);

                            Err::<(), ()>(())
                        });

                        assert!(outer.is_err());

                        cached_selector_element(&child)
                    }))
                    .into_any_element();

                if non_positional {
                    root = root.select(
                        Select::descendants().class("row").reflects(crate::Styled),
                        |element| element.h(px(12.)),
                    );
                }

                root.select(
                    Select::descendants()
                        .class("row")
                        .reflects(crate::Styled)
                        .nth(0),
                    move |element| {
                        record_match(&selected, &element.element);

                        element.h(px(16.))
                    },
                )
            });

            selected.borrow_mut().clear();
            draw_selector_window(window, cx);

            assert_eq!(renders.get(), 4);
            assert_matches(&selected, &["abandoned-first", "cached-row", "cached-row"]);
            assert_cached_selector_size(window, size(px(40.), px(16.)), cx);
        }
    }

    #[crate::test]
    fn cached_selectors_preserve_invalidation_and_cache_transitions(cx: &mut TestAppContext) {
        let settings = Rc::new(Cell::new((None, 40., 0x112233, 80., 0.)));
        let observed = settings.clone();
        let (window, child, renders) = cached_selector_window(cx, move |child, _renders| {
            let (height, width, color, mask_width, inset) = observed.get();
            let mut root = div()
                .h(px(60.))
                .overflow_hidden()
                .child(child.clone().cached(cached_selector_style().w(px(width))))
                .select(Select::this().reflects(crate::Styled), move |element| {
                    element
                        .w(px(mask_width))
                        .pl(px(inset))
                        .text_color(rgb(color))
                });

            if let Some(height) = height {
                root = root.select(
                    Select::descendants()
                        .class("row")
                        .id("cached-row")
                        .reflects(crate::Styled),
                    move |element| element.h(px(height)),
                );
            }

            root
        });

        for (inputs, expected_renders) in [
            ((None, 40., 0x112233, 80., 0.), 0),
            ((Some(12.), 40., 0x112233, 80., 0.), 1),
            ((Some(16.), 40., 0x112233, 80., 0.), 1),
            ((None, 40., 0x112233, 80., 0.), 1),
            ((None, 40., 0x112233, 80., 0.), 0),
            ((None, 48., 0x112233, 80., 0.), 1),
            ((None, 48., 0x112233, 80., 0.), 0),
            ((None, 48., 0x334455, 80., 0.), 1),
            ((None, 48., 0x334455, 30., 0.), 1),
            ((None, 48., 0x334455, 30., 3.), 1),
            ((None, 48., 0x334455, 30., 3.), 0),
        ] {
            settings.set(inputs);
            renders.set(0);
            draw_selector_window(window, cx);

            let (height, width, _color, mask_width, inset) = inputs;
            assert_eq!(renders.get(), expected_renders);
            assert_cached_selector_size(window, size(px(width), px(height.unwrap_or(20.))), cx);
            cx.update_window(window, |_view, window, _cx| {
                let hitbox = window.rendered_frame.hitboxes.last().unwrap();
                assert_eq!(hitbox.bounds.origin.x, px(inset));
                assert_eq!(hitbox.content_mask.bounds.size.width, px(mask_width));
            })
            .unwrap();
        }

        for refresh in [false, true] {
            renders.set(0);
            cx.update_window(window, |_view, window, cx| {
                if refresh {
                    window.refresh();
                } else {
                    child.update(cx, |_child, cx| cx.notify());
                }
            })
            .unwrap();

            draw_selector_window(window, cx);
            assert_eq!(renders.get(), 1);
            assert_cached_selector_size(window, size(px(48.), px(20.)), cx);
        }
    }

    #[crate::test]
    fn selectors_visit_cached_view_contents_on_each_frame(cx: &mut TestAppContext) {
        let selected = Rc::new(Cell::new(0));
        let observed = selected.clone();
        let (window, _child, renders) = cached_selector_window(cx, move |child, _renders| {
            let selected = observed.clone();

            div().child(cached_selector_element(child)).select(
                Select::descendants().class("row").nth(0),
                move |element| {
                    selected.set(selected.get() + 1);

                    element
                },
            )
        });

        selected.set(0);

        for _redraw in 0..2 {
            draw_selector_window(window, cx);
            assert_cached_selector_size(window, size(px(40.), px(20.)), cx);
        }

        assert_eq!(renders.get(), 2);
        assert_eq!(selected.get(), 2);
    }

    #[test]
    #[should_panic(expected = "selector interval must be nonzero")]
    fn rejects_zero_intervals() {
        Select::children().every(0);
    }

    #[crate::test]
    fn nested_retries_restore_positions_and_keep_attempted_callback_logs(cx: &mut TestAppContext) {
        for outer_succeeds in [false, true] {
            let nth = Rc::new(RefCell::new(Vec::new()));
            let every = Rc::new(RefCell::new(Vec::new()));
            let attached = Rc::new(RefCell::new(Vec::new()));

            let nth_matches = nth.clone();
            let every_matches = every.clone();
            let attached_matches = attached.clone();

            let window = cx.add_window(move |_window, _cx| SelectorTestView {
                render: Box::new(move || {
                    let nth_matches = nth_matches.clone();
                    let every_matches = every_matches.clone();
                    let attached_matches = attached_matches.clone();

                    div()
                        .size_full()
                        .child(crate::deferred(crate::container_query(
                            move |bounds, window, cx| {
                                let origin = window.element_offset();
                                let result = window.transact(|window| {
                                    let mut attempted = div()
                                        .child(selector_row("outer-first").class("row"))
                                        .child(deferred_selector_row("attempt-late"))
                                        .into_any_element();
                                    attempted.prepaint_as_root(origin, bounds.into(), window, cx);

                                    let inner = layout_nested_transaction_row(
                                        outer_succeeds,
                                        attached_matches.clone(),
                                        window,
                                        cx,
                                    );

                                    assert_eq!(inner.is_err(), outer_succeeds);
                                    layout_selector_row("outer-last", window, cx);

                                    if outer_succeeds {
                                        return Ok(());
                                    }

                                    Err(())
                                });

                                assert_eq!(result.is_ok(), outer_succeeds);

                                div()
                                    .child(selector_row("committed").class("row"))
                                    .child(selector_row("committed-next").class("row"))
                                    .child(deferred_selector_row("committed-late"))
                            },
                        )))
                        .select(Select::descendants().class("row").nth(1), move |element| {
                            record_match(&nth_matches, &element.element);

                            element
                        })
                        .select(
                            Select::descendants()
                                .class("row")
                                .reflects(crate::Styled)
                                .every(2),
                            move |element| {
                                record_match(&every_matches, &element.element);

                                element.w(px(24.))
                            },
                        )
                }),
            });

            nth.borrow_mut().clear();
            every.borrow_mut().clear();
            attached.borrow_mut().clear();
            draw_selector_window(window.into(), cx);

            assert_matches(&attached, &["inner"]);

            if outer_succeeds {
                assert_matches(&nth, &["inner", "outer-last"]);
                assert_matches(&every, &["outer-first", "committed", "attempt-late"]);
            } else {
                assert_matches(&nth, &["inner", "committed-next"]);
                assert_matches(
                    &every,
                    &["outer-first", "outer-last", "committed", "committed-late"],
                );
            }

            cx.update_window(window.into(), |_view, window, _cx| {
                assert_eq!(
                    window.rendered_frame.deferred_draws.len(),
                    if outer_succeeds { 3 } else { 2 }
                );
                let attempt_bounds = window.rendered_frame.debug_bounds.get("attempt-late");
                assert_eq!(attempt_bounds.is_some(), outer_succeeds);

                if let Some(bounds) = attempt_bounds {
                    assert_eq!(bounds.size.width, px(24.));
                }

                for (label, width) in [
                    ("committed", 24.),
                    ("committed-next", 8.),
                    ("committed-late", if outer_succeeds { 8. } else { 24. }),
                ] {
                    assert_eq!(
                        window.rendered_frame.debug_bounds[label].size.width,
                        px(width)
                    );
                }
            })
            .unwrap();
        }
    }

    struct SelectorView {
        this_count: Rc<Cell<usize>>,
        children_count: Rc<Cell<usize>>,
        descendants_count: Rc<Cell<usize>>,
        filtered_count: Rc<Cell<usize>>,
    }

    impl Render for SelectorView {
        #[allow(unused_variables)]
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let this_count = self.this_count.clone();
            let children_count = self.children_count.clone();
            let descendants_count = self.descendants_count.clone();
            let filtered_count = self.filtered_count.clone();

            div()
                .child(div().id("target").child(div()).class("card"))
                .child(div())
                .child(crate::container_query(|size, window, cx| div()))
                .select(Select::this().reflects(crate::Styled), move |element| {
                    this_count.set(this_count.get() + 1);

                    element.bg(rgb(0x111111))
                })
                .select(Select::children().reflects(crate::Styled), move |element| {
                    children_count.set(children_count.get() + 1);

                    element.bg(rgb(0x222222))
                })
                .select(
                    Select::descendants().reflects(crate::Styled),
                    move |element| {
                        descendants_count.set(descendants_count.get() + 1);

                        element.bg(rgb(0x333333))
                    },
                )
                .select(
                    Select::children()
                        .reflects(crate::reflection::trait_set!(
                            crate::Styled,
                            crate::ParentElement
                        ))
                        .id("target")
                        .class("card"),
                    move |element| {
                        fn require_traits<ElementType>(element: &ElementType)
                        where
                            ElementType: Element + Styled + ParentElement,
                        {
                            std::hint::black_box(element);
                        }

                        require_traits(&element);
                        filtered_count.set(filtered_count.get() + 1);

                        element.bg(rgb(0x444444))
                    },
                )
        }
    }

    #[crate::test]
    #[allow(unused_variables)]
    fn selects_elements_by_scope_and_predicates(cx: &mut TestAppContext) {
        let this_count = Rc::new(Cell::new(0));
        let children_count = Rc::new(Cell::new(0));
        let descendants_count = Rc::new(Cell::new(0));
        let filtered_count = Rc::new(Cell::new(0));
        let window: AnyWindowHandle = cx
            .add_window({
                let this_count = this_count.clone();
                let children_count = children_count.clone();
                let descendants_count = descendants_count.clone();
                let filtered_count = filtered_count.clone();

                move |window, cx| SelectorView {
                    this_count,
                    children_count,
                    descendants_count,
                    filtered_count,
                }
            })
            .into();

        this_count.set(0);
        children_count.set(0);
        descendants_count.set(0);
        filtered_count.set(0);

        cx.update_window(window, |view, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();

        assert_eq!(this_count.get(), 1);
        assert_eq!(children_count.get(), 2);
        assert_eq!(descendants_count.get(), 4);
        assert_eq!(filtered_count.get(), 1);
    }
}
