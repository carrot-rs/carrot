pub use carrot_dap::{
    StartDebuggingRequestArguments, StartDebuggingRequestArgumentsRequest,
    adapters::{DebugAdapterBinary, DebugTaskDefinition, TcpArguments},
};
pub use carrot_task::{
    AttachRequest, BuildTaskDefinition, DebugRequest, DebugScenario, LaunchRequest,
    TaskTemplate as BuildTaskTemplate, TcpArgumentsTemplate,
};
