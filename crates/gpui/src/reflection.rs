//! Trait reflection and typed access to erased elements.
//!
//! [`reflect_trait`] forwards supported borrowed methods to the original concrete element.
//! Provided methods use its override or inherited default, and borrowed returns retain their
//! receiver lifetime. Forwarded signatures accept lifetime parameters, concrete types, and
//! `impl IntoIterator<Item = ConcreteType>` arguments.
//!
//! Provided owned builders run their trait bodies on [`ReflectedElement`], including builders
//! with generic inputs or `Self` returns. Their concrete overrides do not carry through erasure.
//! For concrete behavior, call a compatible borrowed operation from the builder default.
//! Other provided methods can opt into wrapper behavior with `#[reflect(wrapper_default)]`.
//! The macro consumes this setting and rejects it on required methods, in `cfg_attr`, or when
//! duplicated. Unsupported provided signatures otherwise produce a diagnostic.
//!
//! Callable reflected traits reject associated types and all associated constants, even those
//! with defaults. A wrapper type can contain elements whose concrete constants differ. Move a
//! shared constant outside the trait, or use a borrowed getter to expose a concrete value.
//! Supertraits follow the same rules. GPUI style macros are normalized before classification;
//! unknown trait-item macros must generate the entire annotated trait or expose their items
//! directly. Method tables and forwarded implementations preserve direct and nested configuration.

use crate::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};

#[doc(hidden)]
pub use gpui_macros::__collect_reflected_traits;
pub use gpui_macros::{Reflect, reflect_trait, trait_set};
use smallvec::SmallVec;
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    marker::PhantomData,
    panic,
    sync::LazyLock,
};

/// Identifies a trait made available to element reflection.
#[derive(Clone, Copy, Debug)]
pub struct ReflectedTrait {
    /// The fully qualified name of the trait.
    pub name: &'static str,
    type_id: fn() -> TypeId,
    /// Returns this trait's directly inherited reflected traits.
    pub supertraits: fn() -> &'static [ReflectedTrait],
}

impl ReflectedTrait {
    /// Creates a descriptor for a unique trait marker.
    #[doc(hidden)]
    pub const fn new(name: &'static str, type_id: fn() -> TypeId) -> Self {
        Self {
            name,
            type_id,
            supertraits: || &[],
        }
    }

    /// Attaches inheritance emitted by the trait's reflection macro.
    #[doc(hidden)]
    pub const fn with_supertraits(
        mut self,
        supertraits: fn() -> &'static [ReflectedTrait],
    ) -> Self {
        self.supertraits = supertraits;

        self
    }

    fn trait_type_id(&self) -> TypeId {
        (self.type_id)()
    }
}

impl PartialEq for ReflectedTrait {
    fn eq(&self, other: &Self) -> bool {
        self.trait_type_id() == other.trait_type_id()
    }
}

impl Eq for ReflectedTrait {}

/// A typed descriptor for a trait made available to reflection.
#[doc(hidden)]
pub trait ReflectionToken: Copy + 'static {
    /// The reflection group produced when this token is used by itself.
    type Group: ReflectionGroup;
    /// The generated method table for implementations of this trait.
    type Methods: Any + Send + Sync;

    /// The erased runtime descriptor for this trait.
    fn reflected_trait(self) -> ReflectedTrait;
}

impl ReflectionToken for ReflectedTrait {
    type Group = ErasedReflectionGroup;
    type Methods = ();

    fn reflected_trait(self) -> ReflectedTrait {
        self
    }
}

/// The group used when only an erased runtime descriptor is available.
#[doc(hidden)]
pub struct ErasedReflectionGroup;

impl ReflectionGroup for ErasedReflectionGroup {}

/// Marks a type generated to represent a set of reflected traits.
#[doc(hidden)]
pub trait ReflectionGroup: 'static {}

/// Proves that a reflected trait is included in a generated reflection group.
#[doc(hidden)]
pub trait IncludesReflectedTrait<Token>: ReflectionGroup
where
    Token: ReflectionToken,
{
}

/// A value containing the runtime descriptors for a generated reflection group.
#[doc(hidden)]
pub struct ReflectedTraitGroup<Group>
where
    Group: ReflectionGroup,
{
    traits: SmallVec<[ReflectedTrait; 2]>,
    group: PhantomData<fn() -> Group>,
}

impl<Group> ReflectedTraitGroup<Group>
where
    Group: ReflectionGroup,
{
    /// Creates a reflected trait group from its runtime descriptors.
    #[doc(hidden)]
    pub fn new(traits: impl IntoIterator<Item = ReflectedTrait>) -> Self {
        Self {
            traits: traits.into_iter().collect(),
            group: PhantomData,
        }
    }
}

/// Converts one or more typed reflection descriptors into a selector trait set.
#[doc(hidden)]
pub trait ReflectedTraits {
    /// The generated type that records membership of every reflected trait.
    type Group: ReflectionGroup;

    /// Returns the erased runtime descriptors in this trait set.
    fn reflected_traits(self) -> SmallVec<[ReflectedTrait; 2]>;
}

impl<Token> ReflectedTraits for Token
where
    Token: ReflectionToken,
    Token::Group: ReflectionGroup,
{
    type Group = Token::Group;

    fn reflected_traits(self) -> SmallVec<[ReflectedTrait; 2]> {
        std::iter::once(self.reflected_trait()).collect()
    }
}

impl<Group> ReflectedTraits for ReflectedTraitGroup<Group>
where
    Group: ReflectionGroup,
{
    type Group = Group;

    fn reflected_traits(self) -> SmallVec<[ReflectedTrait; 2]> {
        self.traits
    }
}

/// An owned erased element exposing a statically known set of reflected traits.
///
/// Supported borrowed methods dispatch to the original concrete element, including provided
/// defaults and overrides. Owned builders and explicit `#[reflect(wrapper_default)]` methods
/// inherit their bodies on this wrapper and do not dispatch concrete overrides.
#[doc(hidden)]
pub struct ReflectedElement<Group>
where
    Group: ReflectionGroup,
{
    pub(crate) element: AnyElement,
    group: PhantomData<fn() -> Group>,
}

impl<Group> ReflectedElement<Group>
where
    Group: ReflectionGroup,
{
    #[allow(dead_code)]
    pub(crate) fn new(element: AnyElement) -> Self {
        Self {
            element,
            group: PhantomData,
        }
    }

    /// Borrows the original element and its generated method table.
    #[doc(hidden)]
    pub fn __reflection_parts_mut<Token>(
        &mut self,
        token: Token,
    ) -> (&'static Token::Methods, &mut dyn Any)
    where
        Token: ReflectionToken,
        Group: IncludesReflectedTrait<Token>,
    {
        let methods = methods_for(self.element.reflected_type_id(), token);

        (methods, self.element.inner_element_mut())
    }

    /// Borrows the original element and its generated method table.
    #[doc(hidden)]
    pub fn __reflection_parts<Token>(&self, token: Token) -> (&'static Token::Methods, &dyn Any)
    where
        Token: ReflectionToken,
        Group: IncludesReflectedTrait<Token>,
    {
        let methods = methods_for(self.element.reflected_type_id(), token);

        (methods, self.element.inner_element())
    }
}

impl<Group> Element for ReflectedElement<Group>
where
    Group: ReflectionGroup,
{
    type RequestLayoutState = <AnyElement as Element>::RequestLayoutState;
    type PrepaintState = <AnyElement as Element>::PrepaintState;

    fn into_any(self) -> AnyElement {
        self.element
    }

    fn id(&self) -> Option<ElementId> {
        <AnyElement as Element>::id(&self.element)
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        <AnyElement as Element>::source_location(&self.element)
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        <AnyElement as Element>::request_layout(
            &mut self.element,
            global_id,
            inspector_id,
            window,
            cx,
        )
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
        <AnyElement as Element>::prepaint(
            &mut self.element,
            global_id,
            inspector_id,
            bounds,
            request_layout,
            window,
            cx,
        )
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
        <AnyElement as Element>::paint(
            &mut self.element,
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

impl<Group> IntoElement for ReflectedElement<Group>
where
    Group: ReflectionGroup,
{
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }

    fn into_any_element(self) -> AnyElement {
        self.element
    }
}

/// Implemented by `#[derive(Reflect)]` for concrete types whose traits can be inspected.
///
/// The derive detects Styled, InteractiveElement, StatefulInteractiveElement, and ParentElement,
/// including handwritten implementations. All registered traits use generated method tables.
/// Use `#[reflect(MyTrait)]` to expose a custom trait marked with
/// `#[gpui::reflection::reflect_trait]`.
pub trait Reflect: 'static {
    /// Returns the reflected traits implemented by this type.
    fn reflected_traits() -> Vec<ReflectedTrait>;
}

/// A concrete implementation of a reflected trait.
#[doc(hidden)]
pub struct ReflectedImplementation {
    /// The identity and inheritance of the implemented trait.
    pub descriptor: ReflectedTrait,
    /// Its generated forwarding functions.
    pub methods: Box<dyn Any + Send + Sync>,
}

/// A statically linked registration emitted by the reflection derive.
#[doc(hidden)]
pub struct ReflectionRegistration {
    /// Returns the registered concrete type's identity.
    pub type_id: fn() -> TypeId,
    /// Returns its reflected implementations, including inherited traits.
    pub implementations: fn() -> Vec<ReflectedImplementation>,
}

inventory::collect!(ReflectionRegistration);

struct RegisteredTraits {
    descriptors: Vec<ReflectedTrait>,
    methods: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

static REFLECTIONS: LazyLock<HashMap<TypeId, RegisteredTraits>> = LazyLock::new(|| {
    let mut registered = HashMap::new();

    for registration in inventory::iter::<ReflectionRegistration> {
        let entry = registered
            .entry((registration.type_id)())
            .or_insert_with(|| RegisteredTraits {
                descriptors: Vec::new(),
                methods: HashMap::new(),
            });

        for implementation in (registration.implementations)() {
            let trait_id = implementation.descriptor.trait_type_id();

            if entry.methods.contains_key(&trait_id) {
                continue;
            }

            entry.descriptors.push(implementation.descriptor);
            entry.methods.insert(trait_id, implementation.methods);
        }
    }

    registered
});

/// Returns the generated trait descriptors for a concrete registered type.
#[doc(hidden)]
pub fn registered_traits(type_id: TypeId) -> &'static [ReflectedTrait] {
    REFLECTIONS
        .get(&type_id)
        .map(|registration| registration.descriptors.as_slice())
        .unwrap_or(&[])
}

pub(crate) fn traits_for(type_id: TypeId) -> &'static [ReflectedTrait] {
    registered_traits(type_id)
}

pub(crate) fn implements_trait(type_id: TypeId, reflected_trait: ReflectedTrait) -> bool {
    REFLECTIONS.get(&type_id).is_some_and(|registration| {
        registration
            .methods
            .contains_key(&reflected_trait.trait_type_id())
    })
}

fn methods_for<Token: ReflectionToken>(type_id: TypeId, token: Token) -> &'static Token::Methods {
    let descriptor = token.reflected_trait();

    REFLECTIONS
        .get(&type_id)
        .and_then(|registration| registration.methods.get(&descriptor.trait_type_id()))
        .and_then(|methods| methods.downcast_ref())
        .unwrap_or_else(|| {
            panic!(
                "element {type_id:?} has no method table for {}",
                descriptor.name,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Div, Empty, InteractiveElement, ParentElement, StyleRefinement, Styled, div, hsla, rgb,
    };
    use std::{cell::Cell, rc::Rc};

    const TEXT_LABEL: &str = "body";

    mod text {
        #[gpui_macros::reflect_trait]
        pub trait Text {
            fn read<'element>(&'element self, label: &str) -> &'element str;
            fn edit(&mut self, label: &str) -> &mut String;
            fn append(&mut self, characters: impl IntoIterator<Item = char>);
            fn formatter(&'_ self) -> fn(&str) -> &str;

            fn content(&self) -> &str {
                self.read("body")
            }

            fn revise(&mut self) -> &mut String {
                let content = self.edit("body");
                content.push('?');

                content
            }

            #[reflect(wrapper_default)]
            fn suffix(&mut self, suffix: impl AsRef<str>) {
                self.edit("body").push_str(suffix.as_ref());
            }

            #[reflect(wrapper_default)]
            fn wrapper_label(&self) -> &'static str {
                "wrapper"
            }

            #[cfg(all())]
            #[cfg_attr(any(), cfg(any()))]
            fn configured(&self) -> &str {
                self.content()
            }

            #[cfg(any())]
            fn unavailable_direct(&self) -> UnavailableType;

            #[cfg_attr(all(), cfg_attr(all(), cfg(any()), allow(dead_code)))]
            fn unavailable_nested(&self) -> UnavailableType {
                unreachable!()
            }

            #[cfg_attr(all(), cfg(any()))]
            fn unavailable(&mut self) -> UnavailableType;
        }
    }

    mod branches {
        use crate::reflection::tests::text;

        #[gpui_macros::reflect_trait]
        pub trait Left: text::Text {
            fn decorate(mut self, suffix: impl AsRef<str>) -> Self
            where
                Self: Sized,
            {
                self.edit("body").push_str(suffix.as_ref());

                self
            }
        }

        #[gpui_macros::reflect_trait]
        pub trait Right: text::Text {}
    }

    #[gpui_macros::reflect_trait]
    trait Composite: branches::Left + branches::Right + crate::Styled + crate::ParentElement {}

    #[derive(gpui_macros::Reflect, Default)]
    #[reflect(Composite, text::Text, crate::Styled)]
    struct Card {
        text: String,
        calls: Rc<Cell<usize>>,
        style: StyleRefinement,
        children: Vec<AnyElement>,
    }

    impl text::Text for Card {
        fn read<'element>(&'element self, _label: &str) -> &'element str {
            &self.text
        }

        fn edit(&mut self, _label: &str) -> &mut String {
            self.calls.set(self.calls.get() + 1);

            &mut self.text
        }

        fn append(&mut self, characters: impl IntoIterator<Item = char>) {
            self.text.extend(characters);
        }

        fn formatter(&'_ self) -> fn(&str) -> &str {
            |value| value
        }
    }

    #[derive(gpui_macros::Reflect, Default)]
    #[reflect(Composite)]
    struct Panel {
        card: Card,
        text_style: crate::TextStyleRefinement,
    }

    impl text::Text for Panel {
        fn read<'element>(&'element self, label: &str) -> &'element str {
            text::Text::read(&self.card, label)
        }

        fn edit(&mut self, label: &str) -> &mut String {
            text::Text::edit(&mut self.card, label)
        }

        fn append(&mut self, characters: impl IntoIterator<Item = char>) {
            text::Text::append(&mut self.card, characters);
        }

        fn formatter(&'_ self) -> fn(&str) -> &str {
            text::Text::formatter(&self.card)
        }

        fn content(&self) -> &str {
            "panel override"
        }

        fn revise(&mut self) -> &mut String {
            self.card.text.push('!');

            &mut self.card.text
        }

        fn suffix(&mut self, _suffix: impl AsRef<str>) {
            panic!("explicit wrapper defaults must not dispatch concrete overrides");
        }

        fn wrapper_label(&self) -> &'static str {
            "concrete"
        }
    }

    impl branches::Left for Panel {}

    impl branches::Right for Panel {}

    impl Composite for Panel {}

    impl Styled for Panel {
        fn style(&mut self) -> &mut StyleRefinement {
            &mut self.card.style
        }

        fn text_style(&mut self) -> &mut crate::TextStyleRefinement {
            &mut self.text_style
        }
    }

    impl ParentElement for Panel {
        fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
            self.card.children.extend(elements);
        }
    }

    impl branches::Left for Card {
        fn decorate(mut self, suffix: impl AsRef<str>) -> Self {
            self.text.push_str("concrete builder ");
            self.text.push_str(suffix.as_ref());

            self
        }
    }

    impl branches::Right for Card {}

    impl Composite for Card {}

    impl Styled for Card {
        fn style(&mut self) -> &mut StyleRefinement {
            &mut self.style
        }
    }

    impl ParentElement for Card {
        fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
            self.children.extend(elements);
        }
    }

    macro_rules! element_impl {
        ($name:ty) => {
            impl IntoElement for $name {
                type Element = Self;

                fn into_element(self) -> Self {
                    self
                }
            }

            impl Element for $name {
                type RequestLayoutState = ();
                type PrepaintState = ();

                fn id(&self) -> Option<ElementId> {
                    None
                }

                fn source_location(&self) -> Option<&'static panic::Location<'static>> {
                    None
                }

                fn request_layout(
                    &mut self,
                    _id: Option<&GlobalElementId>,
                    _inspector_id: Option<&InspectorElementId>,
                    _window: &mut Window,
                    _cx: &mut App,
                ) -> (LayoutId, ()) {
                    unreachable!()
                }

                fn prepaint(
                    &mut self,
                    _id: Option<&GlobalElementId>,
                    _inspector_id: Option<&InspectorElementId>,
                    _bounds: Bounds<Pixels>,
                    _request_layout: &mut (),
                    _window: &mut Window,
                    _cx: &mut App,
                ) {
                    unreachable!()
                }

                fn paint(
                    &mut self,
                    _id: Option<&GlobalElementId>,
                    _inspector_id: Option<&InspectorElementId>,
                    _bounds: Bounds<Pixels>,
                    _request_layout: &mut (),
                    _prepaint: &mut (),
                    _window: &mut Window,
                    _cx: &mut App,
                ) {
                    unreachable!()
                }
            }
        };
    }

    element_impl!(Card);
    element_impl!(Panel);

    fn card() -> Card {
        Card {
            text: "hello".into(),
            ..Default::default()
        }
    }

    fn transform<Traits>(element: AnyElement, _traits: Traits) -> AnyElement
    where
        Traits: ReflectedTraits,
        ReflectedElement<Traits::Group>: Composite,
    {
        let mut element = ReflectedElement::<Traits::Group>::new(element);
        let label = TEXT_LABEL;

        assert_eq!(text::Text::read(&element, label), "hello");
        text::Text::edit(&mut element, label).push(' ');

        let borrowed = String::from("world");
        let formatter = text::Text::formatter(&element);

        assert_eq!(formatter(&borrowed), "world");
        text::Text::append(&mut element, borrowed.chars());

        branches::Left::decorate(element, "!")
            .bg(rgb(0x123456))
            .invisible()
            .child(Empty)
            .into_any_element()
    }

    #[test]
    fn forwards_methods_and_owned_builders_through_inherited_groups() {
        let concrete = branches::Left::decorate(card(), "!");

        assert_eq!(concrete.text, "helloconcrete builder !");

        for combined in [false, true] {
            let card = card();
            let calls = card.calls.clone();
            let element = card.into_any_element();

            let mut element = if combined {
                transform(
                    element,
                    trait_set![
                        Composite,
                        text::Text,
                        branches::Left,
                        crate::Styled,
                        crate::ParentElement,
                        Composite,
                    ],
                )
            } else {
                transform(element, Composite)
            };

            let card = element.downcast_mut::<Card>().unwrap();

            assert_eq!(card.text, "hello world!");
            assert_eq!(calls.get(), 2);
            assert!(card.style.background.is_some());
            assert_eq!(card.style.visibility, Some(crate::Visibility::Hidden));
            assert_eq!(card.children.len(), 1);
        }
    }

    #[test]
    fn dispatches_defaults_and_overrides_through_one_diamond_group() {
        let color = hsla(0.5, 0.5, 0.5, 1.0);

        for (overrides, content, revised) in [
            (false, "hello", "hello?"),
            (true, "panel override", "hello!"),
        ] {
            let original = if overrides {
                Panel {
                    card: card(),
                    ..Default::default()
                }
                .into_any_element()
            } else {
                card().into_any_element()
            };

            let mut element = ReflectedElement::<__GpuiReflectCompositeGroup>::new(original);

            assert_eq!(text::Text::content(&element), content);
            assert_eq!(text::Text::configured(&element), content);
            assert_eq!(text::Text::revise(&mut element), revised);
            assert_eq!(text::Text::wrapper_label(&element), "wrapper");
            text::Text::suffix(&mut element, " suffix");

            let mut element = element.text_color(color).into_any_element();
            let (card, text_color) = if overrides {
                let panel = element.downcast_mut::<Panel>().unwrap();

                (&panel.card, panel.text_style.color)
            } else {
                let card = element.downcast_mut::<Card>().unwrap();

                (&*card, card.style.text.color)
            };

            assert_eq!(card.text, format!("{revised} suffix"));
            assert_eq!(text_color, Some(color));
        }
    }

    #[test]
    fn records_inheritance_and_deduplicates_concrete_implementations() {
        let descriptor = ReflectionToken::reflected_trait(Composite);
        let parents = (descriptor.supertraits)();

        assert_eq!(parents.len(), 4);
        assert_eq!(
            (parents[0].supertraits)(),
            &[ReflectionToken::reflected_trait(text::Text)],
        );
        assert_eq!(
            (parents[1].supertraits)(),
            &[ReflectionToken::reflected_trait(text::Text)],
        );

        let traits = Card::reflected_traits();
        assert_eq!(traits.len(), 6);
        assert!(traits.contains(&descriptor));
        assert!(traits.contains(&ReflectionToken::reflected_trait(text::Text)));
    }

    #[test]
    fn transparent_erasure_preserves_concrete_identity() {
        fn reflected<Token: ReflectionToken>(
            element: AnyElement,
            _token: Token,
        ) -> ReflectedElement<Token::Group> {
            ReflectedElement::new(element)
        }

        for route in 0..7 {
            let mut element = div().id("original").child(Empty).into_any_element();
            let original = element.downcast_mut::<Div>().unwrap() as *mut Div;
            let reflected = reflected(element, crate::InteractiveElement);
            let mut element = match route {
                0 => reflected.into_any_element(),
                1 => reflected.into_any(),
                2 => reflected.into_element().into_any(),
                3 => reflected.id("renamed").into_any_element(),
                4 => reflected.id("renamed").into_any(),
                5 => reflected.id("renamed").into_element().into_any(),
                _route => reflected.into_any_element().into_any(),
            };

            let expected_id = if (3..6).contains(&route) {
                "renamed"
            } else {
                "original"
            };

            let concrete = element.downcast_mut::<Div>().unwrap();

            assert_eq!(concrete as *mut Div, original);
            assert_eq!(Element::id(concrete), Some(ElementId::from(expected_id)));
            assert_eq!(element.reflected_type_id(), TypeId::of::<Div>());
        }
    }
}
