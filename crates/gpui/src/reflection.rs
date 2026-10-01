//! Trait reflection and typed access to erased elements.

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
#[cfg(test)]
use std::{cell::Cell, rc::Rc};

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
    use crate::{Empty, ParentElement, StyleRefinement, Styled, rgb};

    mod text {
        #[gpui_macros::reflect_trait]
        pub trait Text {
            const LABEL: &'static str = "body";

            fn read<'element>(&'element self, label: &str) -> &'element str;
            fn edit(&mut self, label: &str) -> &mut String;
            fn append(&mut self, characters: impl IntoIterator<Item = char>);
            fn formatter(&'_ self) -> fn(&str) -> &str;

            #[cfg_attr(all(), cfg(any()))]
            fn unavailable(&mut self) -> UnavailableType;
        }
    }

    mod branches {
        #[gpui_macros::reflect_trait]
        pub trait Left: super::text::Text {
            fn decorate(mut self) -> Self
            where
                Self: Sized,
            {
                self.edit("body").push('!');

                self
            }
        }

        #[gpui_macros::reflect_trait]
        pub trait Right: super::text::Text {}
    }

    #[gpui_macros::reflect_trait]
    trait Composite: branches::Left + branches::Right + crate::Styled + crate::ParentElement {}

    #[derive(gpui_macros::Reflect)]
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

    impl branches::Left for Card {}
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

    impl IntoElement for Card {
        type Element = Self;

        fn into_element(self) -> Self {
            self
        }
    }

    impl Element for Card {
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

    fn transform<Traits>(element: AnyElement, _traits: Traits) -> AnyElement
    where
        Traits: ReflectedTraits,
        ReflectedElement<Traits::Group>: Composite,
    {
        let mut element = ReflectedElement::<Traits::Group>::new(element);
        let label = <ReflectedElement<Traits::Group> as text::Text>::LABEL;
        assert_eq!(text::Text::read(&element, label), "hello");
        text::Text::edit(&mut element, label).push(' ');

        let borrowed = String::from("world");
        let formatter = text::Text::formatter(&element);
        assert_eq!(formatter(&borrowed), "world");
        text::Text::append(&mut element, borrowed.chars());

        branches::Left::decorate(element)
            .bg(rgb(0x123456))
            .child(Empty)
            .into_any_element()
    }

    #[test]
    fn forwards_methods_and_defaults_through_inherited_selector_groups() {
        for combined in [false, true] {
            let calls = Rc::new(Cell::new(0));
            let element = Card {
                text: "hello".into(),
                calls: calls.clone(),
                style: StyleRefinement::default(),
                children: Vec::new(),
            }
            .into_any_element();
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
            assert_eq!(card.children.len(), 1);
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
}
