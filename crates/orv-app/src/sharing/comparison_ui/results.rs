// SPDX-License-Identifier: GPL-3.0-or-later
use super::ComparisonUi;
use eframe::egui::{self, RichText};
use orv_agents::RecipientStatus;
use orv_mcp::http::ServerStatus;

impl ComparisonUi {
    pub(super) fn results(&mut self, ctx: &egui::Context) {
        let Some(live) = &mut self.live else {
            return;
        };
        let mut revoke = false;
        crate::dialogs::window("Experimental AI commentary", ctx).id(egui::Id::new(("comparison-results",live.id)))
            .open(&mut live.open).default_width(880.).max_width((ctx.content_rect().width()-32.).max(300.))
            .max_height((ctx.content_rect().height()-48.).max(250.)).collapsible(false).show(ctx,|ui|{
                ui.label("Personal education/research only. Replies may be wrong. Not for diagnosis, treatment or clinical decisions.");
                ui.label("Frozen comparison: moving the viewer does not change the shared image. Close the study or revoke to erase this local comparison.");
                if ui.button("Revoke and clear comparison").clicked(){revoke=true;}
                egui::ScrollArea::vertical().id_salt(("comparison-results-scroll",live.id)).show(ui,|ui|{
                    egui::CollapsingHeader::new("Exact shared image and provenance").show(ui,|ui|{
                        let size=live.texture.size_vec2(); let scale=(180./size.y).min(ui.available_width()/size.x).min(1.);
                        ui.image((live.texture.id(),size*scale));
                        ui.add(egui::Label::new(RichText::new(&live.json).small().monospace()).wrap());
                    });
                    let n=live.comparison.recipients().len();
                    let columns=if ui.available_width()>=680. {n.min(2)}else{1};
                    for start in (0..n).step_by(columns){
                        ui.columns(columns,|uis|{
                            for (column,ui) in uis.iter_mut().enumerate(){
                                let i=start+column;if i>=n {continue;}
                                let recipient=&live.comparison.recipients()[i];let server=&live.servers[i];
                                ui.group(|ui|{
                                    ui.set_min_width(ui.available_width().max(0.));
                                    ui.heading(recipient.label());
                                    match recipient.status(){
                                        RecipientStatus::Waiting=>{ui.label("Waiting for this recipient to read the snapshot.");}
                                        RecipientStatus::SnapshotRead=>{ui.label("Snapshot requested. Waiting for commentary.");}
                                        RecipientStatus::Submitted(reply)=>{
                                            ui.label(format!("Self-reported model: {}",reply.model));
                                            ui.label("Experimental AI commentary · matching scene/PNG hashes");
                                            // Plain egui text only: no Markdown, remote images,
                                            // automatic links, executable commands or rich UI.
                                            ui.add(egui::Label::new(reply.commentary).wrap().selectable(true));
                                        }
                                        RecipientStatus::Revoked=>{ui.label("Access revoked.");}
                                    }
                                    if let ServerStatus::Listening{port}=server.status(){
                                        egui::CollapsingHeader::new("Connect this recipient").id_salt((live.id,i)).show(ui,|ui|{
                                            ui.label("Anyone with this configuration can act as this recipient. Copy only to its local agent; remove it from the agent when finished.");
                                            let endpoint=format!("http://127.0.0.1:{port}/mcp");
                                            ui.label(&endpoint);
                                            if ui.button("Copy HTTP configuration").clicked(){
                                                let key=format!("OpenRadViewComparison{}",i+1);
                                                let config=serde_json::json!({"mcpServers":{(key):{"url":endpoint,"headers":{"Authorization":format!("Bearer {}",server.token_for_configuration())}}}});
                                                if let Ok(text)=serde_json::to_string_pretty(&config){ctx.copy_text(text);}
                                            }
                                            if ui.button("Copy Codex HTTP TOML").clicked()
                                                && let Some(text)=crate::sharing::client_config::codex(&format!("openradview_comparison_{}", i+1), &endpoint, server.token_for_configuration(), None) {ctx.copy_text(text);}
                                            let companion=std::env::current_exe().ok().and_then(|p|p.parent().map(|d|d.join(if cfg!(windows){"openradview-mcp.exe"}else{"openradview-mcp"})));
                                            let has_companion=companion.as_ref().is_some_and(|p|p.is_file());
                                            if ui.add_enabled(has_companion,egui::Button::new("Copy stdio configuration")).on_disabled_hover_text("Use the MCP-enabled app bundle to include the stdio companion.").clicked()
                                                && let Some(ref companion)=companion {
                                                let key=format!("OpenRadViewComparison{}",i+1);
                                                let config=serde_json::json!({"mcpServers":{(key):{"command":companion,"args":["stdio","--url",endpoint],"env":{"OPENRADVIEW_TOKEN":server.token_for_configuration()}}}});
                                                if let Ok(text)=serde_json::to_string_pretty(&config){ctx.copy_text(text);}
                                            }
                                            if ui.add_enabled(has_companion,egui::Button::new("Copy Codex stdio TOML")).clicked()
                                                && let Some(companion)=companion.as_deref()
                                                && let Some(text)=crate::sharing::client_config::codex(&format!("openradview_comparison_{}", i+1), &endpoint, server.token_for_configuration(), Some(companion)) {ctx.copy_text(text);}
                                            if ui.button("Copy agent task").clicked(){
                                                ctx.copy_text(format!("Use only OpenRadViewComparison{}. Call comparison_snapshot, inspect its PNG and manifest, and submit_commentary with the exact scene_sha256 ({}) and png_sha256 ({}) from that manifest, your model label and plain experimental AI commentary. Do not access other files, tools or views.\n\n{}",i+1,live.manifest.scene_sha256,live.manifest.png_sha256,crate::copy::AGENT_GUIDANCE));
                                            }
                                        });
                                    }else{ui.label("Starting local connection…");}
                                });
                            }
                        });
                        ui.add_space(8.);
                    }
                });
            });
        if revoke {
            self.revoke();
            self.notice=Some("Comparison revoked and cleared. Data already received by agents cannot be recalled.".into());
        }
    }
}
