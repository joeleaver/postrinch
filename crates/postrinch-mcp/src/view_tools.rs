//! The views tools: the sidebar's views, as the person sees and changes
//! them with the pencil on a view's row.

use postrinch_agents::Call;
use postrinch_agents::view_shape::ViewShape;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::Postrinch;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveViewArgs {
    /// To change a view: its `view` key or its name, as list_views gives
    /// them. Leave it out to make a new view, which needs a `name`; it
    /// goes after the person's other views.
    #[serde(default)]
    pub view: Option<String>,
    /// What to set. Only what is given changes.
    #[serde(flatten)]
    pub change: ViewShape,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ViewArgs {
    /// The view's `view` key or its name, as list_views gives them.
    pub view: String,
}

#[tool_router(router = view_router, vis = "pub(crate)")]
impl Postrinch {
    /// The views in Postrinch's sidebar, in its order: each one's `view`
    /// key, name, search, accounts (empty for every account), what the
    /// sidebar counts beside it, whether the sidebar lists it, its order,
    /// whether its rows are conversations or single messages, its kind
    /// (built_in, account_inbox or yours), whether you may change it, and
    /// how many conversations it has (and how many unread). Everything
    /// else shows inbox mail no other view does.
    #[tool(name = "list_views")]
    async fn list_views(&self) -> Result<String, String> {
        self.call(Call::Views).await
    }

    /// Make a view, or change one. The person sees it in the sidebar at
    /// once. A built-in view changed this way can be put back with
    /// delete_view. Answers with the view as saved and how many
    /// conversations it now has: check that it shows what was meant.
    #[tool(name = "save_view")]
    async fn save_view(
        &self,
        Parameters(args): Parameters<SaveViewArgs>,
    ) -> Result<String, String> {
        self.call(Call::SaveView {
            view: args.view,
            change: args.change,
        })
        .await
    }

    /// Delete one of the person's views, or put a built-in view back as
    /// it first was. Its mail is not touched. Deleting cannot be undone:
    /// the answer has what it was, for save_view to make it again.
    #[tool(name = "delete_view")]
    async fn delete_view(&self, Parameters(args): Parameters<ViewArgs>) -> Result<String, String> {
        self.call(Call::DeleteView { view: args.view }).await
    }
}
