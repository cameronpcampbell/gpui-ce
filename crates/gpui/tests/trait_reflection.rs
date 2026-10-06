#[cfg(test)]
mod tests {
    use gpui::{
        AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId,
        Interactivity, IntoElement, LayoutId, Pixels, StyleRefinement, Window,
    };

    use gpui::reflection::{Reflect, ReflectionToken};

    #[gpui::reflection::reflect_trait]
    trait Draggable {
        fn distance(&mut self) -> &mut usize;

        fn label(&self) -> &str {
            "default"
        }

        fn drag(&mut self, distance: usize) -> usize {
            let total = self.distance();
            *total += distance;

            *total
        }

        #[cfg_attr(all(), cfg_attr(all(), cfg(any())))]
        fn unavailable(&self) -> UnavailableType;

        fn draggable(self) -> Self
        where
            Self: Sized,
        {
            self
        }
    }

    mod other {
        #[gpui::reflection::reflect_trait]
        pub trait Draggable {}
    }

    mod inherited {
        #[gpui::reflection::reflect_trait]
        pub trait Control: gpui::StatefulInteractiveElement {}

        pub use self::__GpuiReflectControlSchema as __GpuiReflectPublicControlSchema;
        pub use self::Control as PublicControl;
    }

    #[derive(gpui::reflection::Reflect, Default)]
    struct Card {
        style: StyleRefinement,
        children: Vec<AnyElement>,
    }

    impl gpui::Styled for Card {
        fn style(&mut self) -> &mut StyleRefinement {
            &mut self.style
        }
    }

    impl gpui::ParentElement for Card {
        fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
            self.children.extend(elements);
        }
    }

    #[derive(gpui::reflection::Reflect, Default)]
    #[reflect(Draggable, inherited::Control)]
    struct Control {
        interactivity: Interactivity,
        distance: usize,
    }

    impl gpui::InteractiveElement for Control {
        fn interactivity(&mut self) -> &mut Interactivity {
            &mut self.interactivity
        }
    }

    impl gpui::StatefulInteractiveElement for Control {}

    impl Draggable for Control {
        fn distance(&mut self) -> &mut usize {
            &mut self.distance
        }

        fn label(&self) -> &str {
            "control"
        }

        fn drag(&mut self, distance: usize) -> usize {
            self.distance += distance * 2;

            self.distance
        }
    }

    #[derive(gpui::reflection::Reflect, Default)]
    #[reflect(Draggable)]
    struct DefaultControl {
        distance: usize,
    }

    impl Draggable for DefaultControl {
        fn distance(&mut self) -> &mut usize {
            &mut self.distance
        }
    }

    impl inherited::Control for Control {}

    impl other::Draggable for Control {}

    macro_rules! element_impl {
        ($name:ty $(, [$($generics:tt)*])? $(, reflection $reflection:expr)?) => {
            impl $(<$($generics)*>)? IntoElement for $name {
                type Element = Self;

                fn into_element(self) -> Self::Element {
                    self
                }
            }

            impl $(<$($generics)*>)? Element for $name {
                type RequestLayoutState = ();
                type PrepaintState = ();

                $(fn reflection(&self) -> &'static gpui::reflection::ElementReflection {
                    $reflection
                })?

                fn id(&self) -> Option<ElementId> {
                    None
                }

                fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
                    None
                }

                fn request_layout(
                    &mut self,
                    _global_id: Option<&GlobalElementId>,
                    _inspector_id: Option<&InspectorElementId>,
                    _window: &mut Window,
                    _cx: &mut App,
                ) -> (LayoutId, Self::RequestLayoutState) {
                    unreachable!()
                }

                fn prepaint(
                    &mut self,
                    _global_id: Option<&GlobalElementId>,
                    _inspector_id: Option<&InspectorElementId>,
                    _bounds: Bounds<Pixels>,
                    _request_layout_state: &mut Self::RequestLayoutState,
                    _window: &mut Window,
                    _cx: &mut App,
                ) -> Self::PrepaintState {
                    unreachable!()
                }

                fn paint(
                    &mut self,
                    _global_id: Option<&GlobalElementId>,
                    _inspector_id: Option<&InspectorElementId>,
                    _bounds: Bounds<Pixels>,
                    _request_layout_state: &mut Self::RequestLayoutState,
                    _prepaint_state: &mut Self::PrepaintState,
                    _window: &mut Window,
                    _cx: &mut App,
                ) {
                    unreachable!()
                }
            }
        };
    }

    element_impl!(Card);

    element_impl!(Control);

    #[gpui::reflection::reflect_trait(membership)]
    trait PaintSource: std::fmt::Debug + Send {
        type Brush;
        const PALETTE_SIZE: usize;

        fn paint<Input>(&self, input: Input) -> Self::Brush;
    }

    #[cfg(reflection_parent)]
    #[gpui::reflection::reflect_trait]
    trait CallablePaintSource: PaintSource {}

    #[derive(gpui::reflection::Reflect, Debug)]
    #[reflect(gpui::Styled, PaintSource, PaintSource)]
    struct GenericCard<State, const COUNT: usize>
    where
        State: std::fmt::Debug,
    {
        state: State,
        style: StyleRefinement,
    }

    impl<State: std::fmt::Debug + Send, const COUNT: usize> PaintSource for GenericCard<State, COUNT> {
        type Brush = usize;
        const PALETTE_SIZE: usize = COUNT;

        fn paint<Input>(&self, _input: Input) -> usize {
            Self::PALETTE_SIZE
        }
    }

    impl<State: std::fmt::Debug, const COUNT: usize> gpui::Styled for GenericCard<State, COUNT> {
        fn style(&mut self) -> &mut StyleRefinement {
            &mut self.style
        }
    }

    element_impl!(GenericCard<State, COUNT>, [State: std::fmt::Debug + Send + 'static, const COUNT: usize],
        reflection <Self as gpui::reflection::Reflect>::reflection());

    fn methods<Type: Draggable + 'static>() -> Box<__GpuiReflectDraggableMethods> {
        let mut implementations = Vec::new();
        Draggable.__register::<Type>(&mut implementations);

        implementations
            .pop()
            .unwrap()
            .methods
            .unwrap()
            .downcast()
            .unwrap()
    }

    #[test]
    fn exposes_generic_and_membership_registrations_through_erasure() {
        let mut card = GenericCard::<String, 4> {
            state: "state".into(),
            style: StyleRefinement::default(),
        }
        .into_any_element();
        let metadata = card.reflection();

        assert!(card.implements_trait(PaintSource));
        assert!(card.implements_trait(gpui::Styled));
        assert!(metadata.has_adapter(gpui::Styled));
        assert!(!PaintSource.requires_adapter());
        assert!((PaintSource.reflected_trait().supertraits)().is_empty());
        assert_eq!(metadata.descriptors().len(), 2);
        assert!(std::ptr::eq(
            metadata,
            <GenericCard<String, 4> as Reflect>::reflection()
        ));
        assert_eq!(
            gpui::reflection::registered_traits(std::any::TypeId::of::<GenericCard<String, 4>>()),
            metadata.descriptors()
        );

        let concrete = card.downcast_mut::<GenericCard<String, 4>>().unwrap();

        assert_eq!(concrete.state, "state");
        assert_eq!(PaintSource::paint(concrete, "input"), 4);
        assert_eq!(GenericCard::<String, 4>::PALETTE_SIZE, 4);

        let other = GenericCard::<u32, 8> {
            state: 42,
            style: StyleRefinement::default(),
        }
        .into_any_element();

        assert!(!std::ptr::eq(metadata, other.reflection()));
        assert_eq!(
            other.reflection().concrete_type(),
            Some(std::any::TypeId::of::<GenericCard<u32, 8>>())
        );
    }

    #[test]
    fn reflects_traits() {
        fn require_other_draggable<Type: other::Draggable>() {}
        fn require_selected_traits<Traits>(traits: Traits)
        where
            Traits: gpui::reflection::ReflectedTraits,
            gpui::reflection::ReflectedElement<Traits::Group>:
                gpui::Element + gpui::Styled + gpui::ParentElement + Draggable,
        {
            drop(traits);
        }

        fn require_inherited_traits<Traits>(traits: Traits)
        where
            Traits: gpui::reflection::ReflectedTraits,
            gpui::reflection::ReflectedElement<Traits::Group>:
                inherited::Control + gpui::StatefulInteractiveElement + gpui::InteractiveElement,
        {
            drop(traits);
        }

        require_other_draggable::<Control>();
        require_selected_traits(gpui::reflection::trait_set!(
            gpui::Styled,
            gpui::ParentElement,
            crate::tests::Draggable,
        ));
        require_inherited_traits(inherited::Control);
        require_inherited_traits(gpui::reflection::trait_set!(
            inherited::PublicControl,
            gpui::InteractiveElement,
            gpui::StatefulInteractiveElement,
            inherited::Control,
        ));

        let card = Card::default().into_any_element();

        assert!(card.implements_trait(gpui::Styled));
        assert!(card.implements_trait(gpui::ParentElement));
        assert!(!card.implements_trait(gpui::InteractiveElement));
        assert!(!card.implements_trait(Draggable));

        let control = Control::default().draggable().into_any_element();

        assert!(control.implements_trait(gpui::InteractiveElement));
        assert!(control.implements_trait(gpui::StatefulInteractiveElement));
        assert!(control.implements_trait(Draggable));
        assert!(control.implements_trait(inherited::Control));
        assert!(!control.implements_trait(other::Draggable));
        assert!(!control.implements_trait(gpui::Styled));

        let empty = gpui::Empty.into_any_element();

        assert!(empty.reflected_traits().is_empty());
    }

    #[test]
    fn dispatches_registered_defaults_and_overrides() {
        let mut control = Control::default();
        let mut inherited = DefaultControl::default();

        let concrete_methods = methods::<Control>();
        let inherited_methods = methods::<DefaultControl>();

        assert_eq!((concrete_methods.label)(&control), "control");
        assert_eq!((inherited_methods.label)(&inherited), "default");
        assert_eq!((concrete_methods.drag)(&mut control, 3), 6);
        assert_eq!((inherited_methods.drag)(&mut inherited, 3), 3);

        assert_eq!((control.distance, inherited.distance), (6, 3));
    }

    #[cfg(any(
        reflection_bounds,
        reflection_token,
        reflection_method,
        reflection_type
    ))]
    #[test]
    fn rejects_unavailable_membership_capabilities() {
        #[cfg(reflection_bounds)]
        let _metadata = <GenericCard<std::rc::Rc<usize>, 4> as Reflect>::reflection();

        #[cfg(reflection_token)]
        {
            fn require_callable<Token: gpui::reflection::CallableReflectionToken>(_token: Token) {}

            require_callable(PaintSource);
        }

        #[cfg(reflection_method)]
        let _paint = |element: &gpui::reflection::ReflectedElement<
            gpui::reflection::ErasedReflectionGroup,
        >| { PaintSource::paint(element, "input") };

        #[cfg(reflection_type)]
        let _brush: Option<
            <gpui::reflection::ReflectedElement<gpui::reflection::ErasedReflectionGroup> as PaintSource>::Brush,
        > = None;
    }
}
