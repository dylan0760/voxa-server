/// Animate the actual egui layer (including hit-testing), never the saved
/// position or layout of the window. Animations restart on reopening.
pub fn install(ctx: &egui::Context) {
    ctx.on_end_pass(
        "Voxa popup motion",
        std::sync::Arc::new(|ctx| {
            let now = ctx.input(|i| i.time);
            let layers = ctx.memory(|m| m.areas().visible_layer_ids());
            let key = egui::Id::unique("popup-times").with(ctx.viewport_id());
            let mut transitions = Vec::new();
            ctx.data_mut(|d| {
                let times = d.get_temp_mut_or_default::<std::collections::HashMap<egui::LayerId, f64>>(key);
                times.retain(|layer, _| layers.contains(layer));
                for layer in layers.into_iter().filter(|l| matches!(l.order, egui::Order::Middle | egui::Order::Foreground)) {
                    let elapsed = (now - *times.entry(layer).or_insert(now)) / 0.16;
                    transitions.push((layer, ((1.0 - elapsed).clamp(0.0, 1.0).powi(3) * 18.0) as f32));
                }
            });
            for (layer, y) in transitions {
                ctx.set_transform_layer(layer, egui::emath::TSTransform::from_translation(egui::vec2(0.0, y)));
                if y > 0.0 {
                    ctx.request_repaint();
                }
            }
        }),
    );
}
