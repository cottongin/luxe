//! The `debug` tool's schema — the action enum and what each action takes.

use luxe_core::provider::ToolSchema;
use serde_json::json;

pub(super) fn schema() -> ToolSchema {
    ToolSchema {
        name: "debug".to_string(),
        description: "Drive a real debugger (DAP): launch/attach a program, set breakpoints, step, \
                      and inspect/modify state. Prefer this over print-debugging. Rust/C/C++ use \
                      CodeLLDB (or lldb-dap/gdb); Python uses debugpy. Start with launch (or attach); \
                      every follow-up takes the returned `session`. On each stop the result already \
                      includes the top frame + locals. Params by action: \
                      launch{program, args?, cwd?, breakpoints?, stopOnEntry?}; attach{pid | port}; \
                      set_breakpoints{session, path, lines, condition?}; \
                      set_function_breakpoints{session, functions}; \
                      continue|next|step_in|step_out|pause{session, threadId?}; \
                      threads|stack_trace{session}; scopes{session, frameId}; \
                      variables{session, variablesReference}; \
                      set_variable{session, variablesReference, name, value}; \
                      evaluate{session, expression, frameId?}; output|sessions|terminate{session}."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "launch", "attach", "set_breakpoints", "set_function_breakpoints",
                        "continue", "next", "step_in", "step_out", "pause", "threads",
                        "stack_trace", "scopes", "variables", "set_variable", "evaluate",
                        "output", "sessions", "terminate"
                    ],
                    "description": "The debug operation to perform."
                },
                "session": { "type": "string", "description": "Session id (from launch/attach) for follow-up actions." },
                "program": { "type": "string", "description": "launch: path to the program/binary (or .py script)." },
                "args": { "type": "array", "items": { "type": "string" }, "description": "launch: program arguments." },
                "cwd": { "type": "string", "description": "launch/attach: working directory (default: tool cwd)." },
                "adapter": { "type": "string", "description": "force an adapter (debugpy|codelldb|lldb-dap|gdb)." },
                "stopOnEntry": { "type": "boolean", "description": "launch: pause at program entry (default false)." },
                "pid": { "type": "integer", "description": "attach: process id to attach to (native)." },
                "port": { "type": "integer", "description": "attach: port of a listening adapter (debugpy)." },
                "breakpoints": {
                    "type": "array",
                    "description": "launch/set_breakpoints: [{path, lines:[..], conditions?:[..]}] or [{path, breakpoints:[{line, condition?}]}].",
                    "items": { "type": "object" }
                },
                "path": { "type": "string", "description": "set_breakpoints: source file path." },
                "lines": { "type": "array", "items": { "type": "integer" }, "description": "set_breakpoints: 1-based lines." },
                "condition": { "type": "string", "description": "set_breakpoints/set_function_breakpoints: break only when this expression is true." },
                "functions": { "type": "array", "items": { "type": "string" }, "description": "set_function_breakpoints: function names to break on." },
                "threadId": { "type": "integer", "description": "continue/step/pause/stack_trace: thread (default: last stopped)." },
                "frameId": { "type": "integer", "description": "scopes/evaluate: stack frame id." },
                "variablesReference": { "type": "integer", "description": "variables/set_variable: reference from a scope/variable." },
                "name": { "type": "string", "description": "set_variable: variable name." },
                "value": { "type": "string", "description": "set_variable: new value." },
                "expression": { "type": "string", "description": "evaluate: expression to evaluate." }
            },
            "required": ["action"]
        }),
    }
}
