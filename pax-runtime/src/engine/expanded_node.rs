use crate::api::TextInput;
use pax_runtime_api::math::Transform2;
use pax_runtime_api::pax_value::{ImplToFromPaxAny, PaxAny, ToFromPaxAny};
use pax_runtime_api::properties::UntypedProperty;
use pax_runtime_api::{borrow, borrow_mut, use_RefCell, Interpolatable, Property};

use crate::api::math::Point2;
use crate::constants::{
    BUTTON_CLICK_HANDLERS, CHECKBOX_CHANGE_HANDLERS, CLAP_HANDLERS, CLICK_HANDLERS,
    CONTEXT_MENU_HANDLERS, DOUBLE_CLICK_HANDLERS, KEY_DOWN_HANDLERS, KEY_PRESS_HANDLERS,
    KEY_UP_HANDLERS, MOUSE_DOWN_HANDLERS, MOUSE_MOVE_HANDLERS, MOUSE_OUT_HANDLERS,
    MOUSE_OVER_HANDLERS, MOUSE_UP_HANDLERS, SCROLL_HANDLERS, TEXTBOX_CHANGE_HANDLERS,
    TEXTBOX_INPUT_HANDLERS, TEXT_INPUT_HANDLERS, TOUCH_END_HANDLERS, TOUCH_MOVE_HANDLERS,
    TOUCH_START_HANDLERS, WHEEL_HANDLERS,
};
use_RefCell!();
use crate::node_interface::NodeLocal;
use crate::{ExpandedNodeIdentifier, Globals};
use core::fmt;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::api::{
    Axis, ButtonClick, CheckboxChange, Clap, Click, CommonProperties, ContextMenu, DoubleClick,
    Event, KeyDown, KeyPress, KeyUp, MouseDown, MouseMove, MouseOut, MouseOver, MouseUp,
    NodeContext, RenderContext, Scroll, Size, TextboxChange, TextboxInput, TouchEnd, TouchMove,
    TouchStart, Wheel, Window,
};

use crate::{
    compute_tab, ComponentInstance, HandlerLocation, InstanceNode, InstanceNodePtr, RuntimeContext,
    RuntimePropertiesStackFrame,
};

#[derive(Clone)]
pub struct ExpandedNode {
    #[allow(dead_code)]
    /// Unique ID of this expanded node, roughly encoding an address in the tree, where the first u32 is the instance ID
    /// and the subsequent u32s represent addresses within an expanded tree via Repeat.
    pub id: ExpandedNodeIdentifier,

    /// Pointer to the unexpanded `instance_node` underlying this ExpandedNode
    pub instance_node: RefCell<InstanceNodePtr>,

    /// Pointer (`Weak` to avoid Rc cycle memory leaks) to the ExpandedNode directly above
    /// this one.  Used for e.g. event propagation.
    pub parent_expanded_node: RefCell<Weak<ExpandedNode>>,

    /// Id of closest frame present in the node tree.
    /// included as a parameter on AnyCreatePatch when
    /// creating a native element to know what clipping context
    /// to attach to
    pub parent_frame: Cell<Option<ExpandedNodeIdentifier>>,

    /// Reference to the _component for which this `ExpandedNode` is a template member._  Used at least for
    /// getting a reference to slot_children for `slot`.  `Option`al because the very root instance node (root component, root instance node)
    /// has a corollary "root component expanded node."  That very root expanded node _does not have_ a containing ExpandedNode component,
    /// thus `containing_component` is `Option`al.
    pub containing_component: Weak<ExpandedNode>,

    /// Persistent clone of the state of the [`PropertiesTreeShared#runtime_properties_stack`] at the time that this node was expanded (this is expected to remain immutable
    /// through the lifetime of the program after the initial expansion; however, if that constraint changes, this should be
    /// explicitly updated to accommodate.)
    pub stack: Rc<RuntimePropertiesStackFrame>,

    /// Pointers to the ExpandedNode beneath this one.  Used for e.g. rendering recursion.
    pub children: Property<Vec<Rc<ExpandedNode>>>,

    /// A list of mounted children that need to be dismounted when children is recalculated
    pub mounted_children: RefCell<Vec<Rc<ExpandedNode>>>,

    /// Each ExpandedNode has a unique "stamp" of computed properties
    pub properties: RefCell<Rc<RefCell<PaxAny>>>,

    /// Each ExpandedNode has unique, computed `CommonProperties`
    common_properties: RefCell<Rc<RefCell<CommonProperties>>>,
    /// Set by chassis, for for example text nodes that get resize info from an interrupt
    /// if a node doesn't have fixed bounds(width/height specified), this value is used instead.
    pub rendered_size: Property<Option<(f64, f64)>>,

    /// Properties that are currently re-computed each frame before rendering.
    /// Only contains computed_tab atm. Might be possible to retire if tab comp
    /// would be part of render pass?
    pub layout_properties: LayoutProperties,

    /// For component instances only, tracks the expanded slot_children in it's
    /// non-collapsed form (repeat and conditionals still present). This allows
    /// repeat/conditionals to update their children (handled in component.rs
    /// update_children method)
    pub expanded_slot_children: RefCell<Option<Vec<Rc<ExpandedNode>>>>,
    /// Flattened version of the above, where repeat/conditionals are removed
    /// recursively and replaced by their children. This is re-computed each
    /// frame from the non-collapsed expanded_slot_children after they have
    /// been updated.
    pub expanded_and_flattened_slot_children: RefCell<Option<Vec<Rc<ExpandedNode>>>>,
    // Number of expanded and flattened slot children
    pub flattened_slot_children_count: Property<usize>,

    /// Flag that is > 0 if this node is part of the root tree. If it is,
    /// updates to this nodes children also marks them as attached (+1), triggering
    /// mount and dismount on addition/removal. This is needed mainly for slot,
    /// since an entire "shadow tree" needs to be expanded and updated for
    /// each slot child, but only the ones that have a "connected" slot should
    /// trigger mount/dismount updates
    pub attached: RefCell<u32>,

    /// Occlusion layer for this node. Used by canvas elements to decide what canvas to draw on, and
    /// by native elements to move to the correct native layer.
    pub occlusion_id: RefCell<u32>,

    /// A map of all properties available on this expanded node.
    /// Used by the RuntimePropertiesStackFrame to resolve symbols.
    pub properties_scope: RefCell<HashMap<String, UntypedProperty>>,
}

impl ImplToFromPaxAny for ExpandedNode {}
impl Interpolatable for ExpandedNode {}

macro_rules! dispatch_event_handler {
    ($fn_name:ident, $arg_type:ty, $handler_key:ident, $recurse:expr) => {
        pub fn $fn_name(
            &self,
            args: $arg_type,
            globals: &Globals,
            ctx: &Rc<RuntimeContext>,
        ) -> bool {
            let event = Event::new(args.clone());
            if let Some(registry) = borrow!(self.instance_node).base().get_handler_registry() {
                let component_properties = if let Some(cc) = self.containing_component.upgrade() {
                    Rc::clone(&*borrow!(cc.properties))
                } else {
                    Rc::clone(&*borrow!(self.properties))
                };

                let context = self.get_node_context(ctx);
                let borrowed_registry = &borrow!(*registry);
                if let Some(handlers) = borrowed_registry.handlers.get($handler_key) {
                    handlers.iter().for_each(|handler| {
                        let properties = if let HandlerLocation::Component = &handler.location {
                            Rc::clone(&*borrow!(self.properties))
                        } else {
                            Rc::clone(&component_properties)
                        };
                        (handler.function)(
                            Rc::clone(&properties),
                            &context,
                            Some(event.clone().to_pax_any()),
                        );
                    });
                };
            }

            if $recurse {
                if let Some(parent) = borrow!(self.parent_expanded_node).upgrade() {
                    let parent_prevent_default = parent.$fn_name(args, globals, ctx);
                    return event.cancelled() || parent_prevent_default;
                }
            }
            event.cancelled()
        }
    };
}

impl ExpandedNode {
    pub fn root(template: Rc<ComponentInstance>, ctx: &Rc<RuntimeContext>) -> Rc<Self> {
        let root_env = RuntimePropertiesStackFrame::new(
            HashMap::new(),
            Rc::new(RefCell::new(().to_pax_any())),
        );
        let root_node = Self::new(template, root_env, ctx, Weak::new());
        Rc::clone(&root_node).recurse_mount(ctx);
        let globals = ctx.globals();
        let parent_bounds = globals.viewport.bounds.clone();
        let parent_transform = globals.viewport.transform.clone();
        let (transform, bounds) = compute_tab(&root_node, parent_transform, parent_bounds);
        root_node.layout_properties.bounds.replace_with(bounds);
        root_node
            .layout_properties
            .transform
            .replace_with(transform);
        root_node
    }

    fn new(
        template: Rc<dyn InstanceNode>,
        env: Rc<RuntimePropertiesStackFrame>,
        context: &Rc<RuntimeContext>,
        containing_component: Weak<ExpandedNode>,
    ) -> Rc<Self> {
        let properties = (&template.base().instance_prototypical_properties_factory)(
            env.clone(),
            context.expression_table(),
        );
        let common_properties = (&template
            .base()
            .instance_prototypical_common_properties_factory)(
            env.clone(),
            context.expression_table(),
        );

        let mut property_scope = borrow!(*common_properties).retrieve_property_scope();

        if let Some(scope) = &template.base().properties_scope_factory {
            property_scope.extend(scope(properties.clone()));
        }

        let id = context.gen_uid();
        let res = Rc::new(ExpandedNode {
            id,
            stack: env,
            instance_node: RefCell::new(Rc::clone(&template)),
            attached: RefCell::new(0),
            properties: RefCell::new(properties),
            common_properties: RefCell::new(common_properties),
            rendered_size: Property::default(),

            // these two refer to their rendering parent, not their
            // template parent
            parent_expanded_node: Default::default(),
            parent_frame: Default::default(),

            containing_component,
            children: Property::new_with_name(
                Vec::new(),
                &format!("node children (node id: {})", id.0),
            ),
            mounted_children: RefCell::new(Vec::new()),
            layout_properties: LayoutProperties::default(),
            expanded_slot_children: Default::default(),
            expanded_and_flattened_slot_children: Default::default(),
            flattened_slot_children_count: Property::new(0),
            occlusion_id: RefCell::new(0),
            properties_scope: RefCell::new(property_scope),
        });
        res
    }

    pub fn recreate_with_new_data(
        self: &Rc<Self>,
        template: Rc<dyn InstanceNode>,
        context: &Rc<RuntimeContext>,
    ) {
        Rc::clone(self).recurse_unmount(context);
        let new_expanded_node = Self::new(
            template.clone(),
            Rc::clone(&borrow!(self.parent_expanded_node).upgrade().unwrap().stack),
            context,
            Weak::clone(&self.containing_component),
        );
        *borrow_mut!(self.instance_node) = Rc::clone(&*borrow!(new_expanded_node.instance_node));
        *borrow_mut!(self.properties) = Rc::clone(&*borrow!(new_expanded_node.properties));
        *borrow_mut!(self.properties_scope) = borrow!(new_expanded_node.properties_scope).clone();
        *borrow_mut!(self.common_properties) =
            Rc::clone(&*borrow!(new_expanded_node.common_properties));

        Rc::clone(self).recurse_mount(context);
        self.bind_to_parent_bounds();
    }

    /// Returns whether this node is a descendant of the ExpandedNode described by `other_expanded_node_id` (id)
    /// Currently requires traversing linked list of ancestory, incurring a O(log(n)) cost for a tree of `n` elements.
    /// This could be mitigated with caching/memoization, perhaps by storing a HashSet on each ExpandedNode describing its ancestory chain.
    pub fn is_descendant_of(&self, other_expanded_node_id: &ExpandedNodeIdentifier) -> bool {
        if let Some(parent) = borrow!(self.parent_expanded_node).upgrade() {
            // We have a parent — if it matches the ID, this node is indeed an ancestor of other_expanded_node_id.  Otherwise, recurse upward.
            if parent.id.eq(other_expanded_node_id) {
                true
            } else {
                parent.is_descendant_of(other_expanded_node_id)
            }
        } else {
            false
        }
    }

    pub fn create_children_detached(
        self: &Rc<Self>,
        templates: impl IntoIterator<Item = (Rc<dyn InstanceNode>, Rc<RuntimePropertiesStackFrame>)>,
        context: &Rc<RuntimeContext>,
    ) -> Vec<Rc<ExpandedNode>> {
        let containing_component = if borrow!(self.instance_node).base().flags().is_component {
            Rc::downgrade(&self)
        } else {
            Weak::clone(&self.containing_component)
        };

        let mut children = Vec::new();

        for (template, env) in templates {
            children.push(Self::new(
                template,
                env,
                context,
                Weak::clone(&containing_component),
            ));
        }
        children
    }

    pub fn attach_children(
        self: &Rc<Self>,
        new_children: Vec<Rc<ExpandedNode>>,
        context: &Rc<RuntimeContext>,
    ) -> Vec<Rc<ExpandedNode>> {
        let mut curr_children = borrow_mut!(self.mounted_children);
        //TODO here we could probably check intersection between old and new children (to avoid unmount + mount)
        for child in new_children.iter() {
            // set parent and connect up viewport bounds to new parent
            *borrow_mut!(child.parent_expanded_node) = Rc::downgrade(self);
        }
        if *borrow!(self.attached) > 0 {
            for child in curr_children.iter() {
                Rc::clone(child).recurse_unmount(context);
            }
            for child in new_children.iter() {
                // set frame clipping reference
                child.bind_to_parent_bounds();
                child.parent_frame.set(self.parent_frame.get());
                Rc::clone(child).recurse_mount(context);
            }
        }
        *curr_children = new_children.clone();
        new_children
    }

    fn bind_to_parent_bounds(self: &Rc<Self>) {
        let parent = borrow!(self.parent_expanded_node).upgrade().unwrap();
        let parent_bounds = parent.layout_properties.bounds.clone();
        let parent_transform = parent.layout_properties.transform.clone();
        let (transform, bounds) = compute_tab(&self, parent_transform, parent_bounds);
        self.layout_properties.bounds.replace_with(bounds);
        self.layout_properties.transform.replace_with(transform);
    }

    pub fn generate_children(
        self: &Rc<Self>,
        templates: impl IntoIterator<Item = (Rc<dyn InstanceNode>, Rc<RuntimePropertiesStackFrame>)>,
        context: &Rc<RuntimeContext>,
    ) -> Vec<Rc<ExpandedNode>> {
        let new_children = self.create_children_detached(templates, context);
        let res = self.attach_children(new_children, context);
        res
    }

    /// This method recursively updates all node properties. When dirty-dag exists, this won't
    /// need to be here since all property dependencies can be set up and removed during mount/unmount
    pub fn recurse_update(self: &Rc<Self>, context: &Rc<RuntimeContext>) {
        if let Some(ref registry) = borrow!(self.instance_node).base().handler_registry {
            for handler in borrow!(registry)
                .handlers
                .get("tick")
                .unwrap_or(&Vec::new())
            {
                (handler.function)(
                    Rc::clone(&*borrow!(self.properties)),
                    &self.get_node_context(context),
                    None,
                )
            }
        }
        Rc::clone(&*borrow!(self.instance_node)).update(&self, context);
        if let Some(ref registry) = borrow!(self.instance_node).base().handler_registry {
            for handler in borrow!(registry)
                .handlers
                .get("pre_render")
                .unwrap_or(&Vec::new())
            {
                (handler.function)(
                    Rc::clone(&*borrow!(self.properties)),
                    &self.get_node_context(context),
                    None,
                )
            }
        }
        for child in self.children.get().iter() {
            child.recurse_update(context);
        }
    }

    pub fn recurse_mount(self: &Rc<Self>, context: &Rc<RuntimeContext>) {
        if *borrow!(self.attached) == 0 {
            *borrow_mut!(self.attached) += 1;
            context.add_to_cache(&self);
            //Note on subtle sequencing here:
            // (1) _primitive_ handle_mounts must fire before updating properties for the first time.
            //     This is at least to appease the needs of Component + Slot ordering; see ComponentInstance#update
            // (2) separately, in userland, we want `mount` events to have properties available for reading & writing.
            //     this requires calling `update` at least once.  (Note that this means `update` is currently called twice on init)
            //Thus: primitive#handle_mount, primitive#update, component#handle_mount.  This requires the primitive author to
            //  be aware of the fact that properties don't yet exist on mount.
            borrow!(self.instance_node)
                .clone()
                .handle_mount(&self, context);
            Rc::clone(&*borrow!(self.instance_node)).update(&self, context);
            if let Some(ref registry) = borrow!(self.instance_node).base().handler_registry {
                for handler in borrow!(registry)
                    .handlers
                    .get("mount")
                    .unwrap_or(&Vec::new())
                {
                    (handler.function)(
                        Rc::clone(&*borrow!(self.properties)),
                        &self.get_node_context(context),
                        None,
                    )
                }
            }
        }
        for child in self.children.get().iter() {
            Rc::clone(child).recurse_mount(context);
        }
    }

    pub fn recurse_unmount(self: Rc<Self>, context: &Rc<RuntimeContext>) {
        // WARNING: do NOT make recurse_unmount result in expr evaluation,
        // in this case: do not refer to self.children expression.
        // expr evaluation in this context can trigger get's of "old data", ie try to get
        // an index of a for loop source that doesn't exist anymore
        for child in borrow!(self.mounted_children).iter() {
            Rc::clone(child).recurse_unmount(context);
        }
        if *borrow!(self.attached) == 1 {
            *borrow_mut!(self.attached) -= 1;
            context.remove_from_cache(&self);
            borrow!(self.instance_node).handle_unmount(&self, context);
        }
    }

    pub fn recurse_render(&self, ctx: &Rc<RuntimeContext>, rcs: &mut dyn RenderContext) {
        borrow!(self.instance_node).handle_pre_render(&self, ctx, rcs);
        for child in self.children.get().iter().rev() {
            child.recurse_render(ctx, rcs);
        }
        borrow!(self.instance_node).render(&self, ctx, rcs);
        borrow!(self.instance_node).handle_post_render(&self, ctx, rcs);
    }

    /// Manages unpacking an Rc<RefCell<PaxValue>>, downcasting into
    /// the parameterized `target_type`, and executing a provided closure `body` in the
    /// context of that unwrapped variant (including support for mutable operations),
    /// the closure is executed.  Used at least by calculating properties in `expand_node` and
    /// passing `&mut self` into event handlers (where the typed `self` is retrieved from an instance of `PaxValue`)
    pub fn with_properties_unwrapped<T: ToFromPaxAny, R>(
        &self,
        callback: impl FnOnce(&mut T) -> R,
    ) -> R {
        // Borrow the contents of the RefCell mutably.
        let properties = borrow_mut!(self.properties);
        let mut borrowed = borrow_mut!(properties);

        // Downcast the unwrapped value to the specified `target_type` (or panic)
        let mut unwrapped_value = if let Ok(val) = T::mut_from_pax_any(&mut *borrowed) {
            val
        } else {
            panic!() //Failed to downcast
        };
        callback(&mut unwrapped_value)
    }

    pub fn recurse_visit_postorder(self: &Rc<Self>, func: &mut impl FnMut(&Rc<Self>)) {
        for child in self.children.get().iter().rev() {
            child.recurse_visit_postorder(func)
        }
        func(self);
    }

    pub fn get_node_context<'a>(&'a self, ctx: &Rc<RuntimeContext>) -> NodeContext {
        let globals = ctx.globals();
        let bounds_self = self.layout_properties.bounds.clone();
        let bounds_parent = if let Some(parent) = borrow!(self.parent_expanded_node).upgrade() {
            parent.layout_properties.bounds.clone()
        } else {
            globals.viewport.bounds.clone()
        };

        let slot_children_count = if borrow!(self.instance_node).base().flags().is_component {
            self.flattened_slot_children_count.clone()
        } else {
            self.containing_component
                .upgrade()
                .map(|v| v.flattened_slot_children_count.clone())
                .unwrap_or_default()
        };

        NodeContext {
            frames_elapsed: globals.frames_elapsed.clone(),
            bounds_self,
            bounds_parent,
            runtime_context: ctx.clone(),
            slot_children_count,
            platform: globals.platform.clone(),
            os: globals.os.clone(),
            #[cfg(feature = "designtime")]
            designtime: globals.designtime.clone(),
        }
    }

    pub fn get_common_properties(&self) -> Rc<RefCell<CommonProperties>> {
        Rc::clone(&*borrow!(self.common_properties))
    }

    /// Determines whether the provided ray, orthogonal to the view plane,
    /// intersects this `ExpandedNode`.
    pub fn ray_cast_test(&self, ray: Point2<Window>) -> bool {
        // Don't vacuously hit for `invisible_to_raycasting` nodes
        if borrow!(self.instance_node)
            .base()
            .flags()
            .invisible_to_raycasting
        {
            return false;
        }

        let inverted_transform = self.layout_properties.transform.get().inverse();
        let transformed_ray = inverted_transform * ray;

        let (width, height) = self.layout_properties.bounds.get();
        //Default implementation: rectilinear bounding hull
        let res = transformed_ray.x > 0.0
            && transformed_ray.y > 0.0
            && transformed_ray.x < width
            && transformed_ray.y < height;
        res
    }

    /// Used at least by ray-casting; only nodes that clip content (and thus should
    /// not allow outside content to respond to ray-casting) should return a value
    pub fn get_clipping_size(&self) -> Option<(Size, Size)> {
        None
    }

    /// Returns the clipping bounds of this node in pixels, requiring
    /// parent bounds for calculation of `Percent` values
    pub fn get_clipping_size_computed(&self, bounds: (f64, f64)) -> (f64, f64) {
        match self.get_clipping_size() {
            None => bounds,
            Some(size_raw) => (
                size_raw.0.evaluate(bounds, Axis::X),
                size_raw.1.evaluate(bounds, Axis::Y),
            ),
        }
    }

    /// Returns the scroll offset from a Scroller component
    /// Used by the engine to transform its children
    pub fn get_scroll_offset(&mut self) -> (f64, f64) {
        // (0.0, 0.0)
        todo!("patch into an ExpandedNode-friendly way to track this state");
    }

    pub fn compute_flattened_slot_children(&self) {
        // All of this should ideally be reactively updated,
        // but currently doesn't exist a way to "listen to"
        // an entire node tree, and generate the flattened list
        // only when changed.
        if let Some(slot_children) = borrow!(self.expanded_slot_children).as_ref() {
            let mut exp_and_flat_slot_children =
                borrow_mut!(self.expanded_and_flattened_slot_children);
            let flattened = flatten_expanded_nodes_for_slot(&slot_children);
            if self.flattened_slot_children_count.get() != flattened.len() {
                self.flattened_slot_children_count.set(flattened.len());
            }
            *exp_and_flat_slot_children = Some(flattened);
        }
    }

    dispatch_event_handler!(dispatch_scroll, Scroll, SCROLL_HANDLERS, true);
    dispatch_event_handler!(dispatch_clap, Clap, CLAP_HANDLERS, true);
    dispatch_event_handler!(dispatch_touch_start, TouchStart, TOUCH_START_HANDLERS, true);

    dispatch_event_handler!(dispatch_touch_move, TouchMove, TOUCH_MOVE_HANDLERS, true);
    dispatch_event_handler!(dispatch_touch_end, TouchEnd, TOUCH_END_HANDLERS, true);
    dispatch_event_handler!(dispatch_key_down, KeyDown, KEY_DOWN_HANDLERS, false);
    dispatch_event_handler!(dispatch_key_up, KeyUp, KEY_UP_HANDLERS, false);
    dispatch_event_handler!(dispatch_key_press, KeyPress, KEY_PRESS_HANDLERS, false);
    dispatch_event_handler!(
        dispatch_checkbox_change,
        CheckboxChange,
        CHECKBOX_CHANGE_HANDLERS,
        true
    );
    dispatch_event_handler!(
        dispatch_textbox_change,
        TextboxChange,
        TEXTBOX_CHANGE_HANDLERS,
        true
    );
    dispatch_event_handler!(dispatch_text_input, TextInput, TEXT_INPUT_HANDLERS, true);
    dispatch_event_handler!(
        dispatch_textbox_input,
        TextboxInput,
        TEXTBOX_INPUT_HANDLERS,
        true
    );
    dispatch_event_handler!(
        dispatch_button_click,
        ButtonClick,
        BUTTON_CLICK_HANDLERS,
        true
    );
    dispatch_event_handler!(dispatch_mouse_down, MouseDown, MOUSE_DOWN_HANDLERS, true);
    dispatch_event_handler!(dispatch_mouse_up, MouseUp, MOUSE_UP_HANDLERS, true);
    dispatch_event_handler!(dispatch_mouse_move, MouseMove, MOUSE_MOVE_HANDLERS, true);
    dispatch_event_handler!(dispatch_mouse_over, MouseOver, MOUSE_OVER_HANDLERS, true);
    dispatch_event_handler!(dispatch_mouse_out, MouseOut, MOUSE_OUT_HANDLERS, true);
    dispatch_event_handler!(
        dispatch_double_click,
        DoubleClick,
        DOUBLE_CLICK_HANDLERS,
        true
    );
    dispatch_event_handler!(
        dispatch_context_menu,
        ContextMenu,
        CONTEXT_MENU_HANDLERS,
        true
    );
    dispatch_event_handler!(dispatch_click, Click, CLICK_HANDLERS, true);
    dispatch_event_handler!(dispatch_wheel, Wheel, WHEEL_HANDLERS, true);

    // fired if the chassis thinks this element should be a different size (in pixels).
    // usually, this is something the engine has asked for, ie. text nodes
    // changing size.
    pub fn chassis_resize_request(self: &Rc<ExpandedNode>, width: f64, height: f64) {
        self.rendered_size.set(Some((width, height)));
    }
}

/// Properties that are currently re-computed each frame before rendering.

#[derive(Debug, Default, Clone)]
pub struct LayoutProperties {
    pub transform: Property<Transform2<NodeLocal, Window>>,
    pub bounds: Property<(f64, f64)>,
}

impl LayoutProperties {
    pub fn corners(&self) -> [Point2<Window>; 4] {
        let (width, height) = self.bounds.get();

        let top_left = self.transform.get() * Point2::new(0.0, 0.0);
        let top_right = self.transform.get() * Point2::new(width, 0.0);
        let bottom_left = self.transform.get() * Point2::new(0.0, height);
        let bottom_right = self.transform.get() * Point2::new(width, height);

        let res = [top_left, top_right, bottom_right, bottom_left];
        res
    }

    //Applies the separating axis theorem to determine whether two `TransformAndBounds` intersect.
    pub fn intersects(&self, other: &Self) -> bool {
        let corners_self = self.corners();
        let corners_other = other.corners();

        for i in 0..2 {
            let axis = (corners_self[i] - corners_self[(i + 1) % 4]).normal();

            let self_projections: Vec<_> = corners_self
                .iter()
                .map(|&p| p.to_vector().project_onto(axis).length())
                .collect();
            let other_projections: Vec<_> = corners_other
                .iter()
                .map(|&p| p.to_vector().project_onto(axis).length())
                .collect();

            let (min_self, max_self) = min_max_projections(&self_projections);
            let (min_other, max_other) = min_max_projections(&other_projections);

            // Check for non-overlapping projections
            if max_self < min_other || max_other < min_self {
                // By the separating axis theorem, non-overlap of projections on _any one_ of the axis-normals proves that these polygons do not intersect.
                return false;
            }
        }
        true
    }
}

fn min_max_projections(projections: &[f64]) -> (f64, f64) {
    let min_projection = *projections
        .iter()
        .min_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap();
    let max_projection = *projections
        .iter()
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap();
    (min_projection, max_projection)
}

/// Given some InstanceNodePtrList, distill away all "slot-invisible" nodes (namely, `if` and `for`)
/// and return another InstanceNodePtrList with a flattened top-level list of nodes.
fn flatten_expanded_nodes_for_slot(nodes: &[Rc<ExpandedNode>]) -> Vec<Rc<ExpandedNode>> {
    let mut result: Vec<Rc<ExpandedNode>> = vec![];
    for node in nodes {
        if borrow!(node.instance_node).base().flags().invisible_to_slot {
            result.extend(flatten_expanded_nodes_for_slot(
                node.children
                    .get()
                    .clone()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .as_slice(),
            ));
        } else {
            result.push(Rc::clone(&node))
        }
    }
    result
}

impl std::fmt::Debug for ExpandedNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        //see: https://users.rust-lang.org/t/reusing-an-fmt-formatter/8531/4
        //maybe this utility should be moved to a more accessible place?
        pub struct Fmt<F>(pub F)
        where
            F: Fn(&mut fmt::Formatter) -> fmt::Result;

        impl<F> fmt::Debug for Fmt<F>
        where
            F: Fn(&mut fmt::Formatter) -> fmt::Result,
        {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                (self.0)(f)
            }
        }

        f.debug_struct("ExpandedNode")
            .field(
                "instance_node",
                &Fmt(|f| borrow!(self.instance_node).resolve_debug(f, Some(self))),
            )
            .field("id", &self.id)
            .field("common_properties", &borrow!(self.common_properties))
            .field("layout_properties", &self.layout_properties)
            .field("children", &self.children.get().iter().collect::<Vec<_>>())
            .field("parent", &borrow!(self.parent_expanded_node).upgrade())
            .field(
                "slot_children",
                &borrow!(self.expanded_and_flattened_slot_children)
                    .as_ref()
                    .map(|o| o.iter().map(|v| v.id).collect::<Vec<_>>()),
            )
            .field("occlusion_id", &borrow!(self.occlusion_id))
            .field(
                "containing_component",
                &self.containing_component.upgrade().map(|v| v.id.clone()),
            )
            .finish()
    }
}
