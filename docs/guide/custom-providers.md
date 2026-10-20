---
title: "Custom providers"
summary: "Connect any endpoint, and set a model's context window, thinking levels, request format and samplers."
order: 6
group: "Extend"
---

luxe knows how to reach every provider in [Models and providers](providers.md), and the
[models.dev](https://models.dev) catalog tells it each model's context window, output limit,
thinking levels and price. This page is for what neither covers: your own endpoint, a model the
catalog does not know, or a local server that needs its requests shaped a particular way.

Everything here goes in `~/.config/luxe/models.json`. `luxe models show <provider>/<id>` shows what
luxe resolved for a model, and where each value came from.

## Add an endpoint

**Connect a local or custom model** in **Configuration** asks for an endpoint and writes it here.
By hand:

```json title="~/.config/luxe/models.json"
{
  "providers": {
    "gateway": {
      "base_url": "https://llm.internal.example.com/v1",
      "api_key": "$GATEWAY_KEY",
      "api": "openai-completions"
    }
  },
  "models": {
    "gateway/qwen3-coder-480b": { "context_window": 262144, "max_tokens": 65536 }
  }
}
```

Then `luxe --model gateway/qwen3-coder-480b`, or choose it in `/model`.

| Provider key | Does |
| --- | --- |
| `base_url` | The endpoint, including its version, such as `/v1` |
| `api_key` | The key: the value, `$VAR`, `${VAR}` or `$env:VAR`, or `!command`, which runs each time a request needs the key. An unset `${VAR}` is empty, as in `mcp.json`, and luxe logs which one. |
| `api` | The API it speaks: `openai-completions` (the default), `openai-responses`, `anthropic-messages` |
| `auth` | How the key is sent: `bearer`, `x-api-key`, `api-key`, or `foundry` for both of the last two |
| `models_dev` | The models.dev provider to take model details from, when the names differ |
| `compat` | How requests are shaped; see [Request format](#request-format) |
| `sampling_params` | Samplers for every model it serves; see [Samplers](#samplers) |

A provider in `models.json` replaces the same provider's block in `auth.json` whole, so keep a
provider's key and endpoint in one file. A provider luxe already knows keeps everything you do not
write. A key in a provider luxe knows is used when its own variable, such as `OPENAI_API_KEY`, is
unset; for one it does not know, the key you write comes first and `OPENAI_API_KEY` is the
fallback. A key you signed in with comes before either.

A file luxe cannot parse is skipped with a warning in the log, and a misspelled key is ignored, so
check the result with `luxe models show <provider>/<id>` after editing.

## Model settings

Settings for one model go under `models`, by `provider/id`. Each value overrides the catalog's,
and everything you leave out still comes from it. `thinking_level_map`, `modalities` and `cost`
each replace the catalog's whole: a `cost` you write drops the catalog's long-prompt prices, and an
`output` price you leave out is free.

| Key | Does |
| --- | --- |
| `context_window` | The model's context window, in tokens |
| `max_tokens` | The most it may write in one reply. Defaults to the model's own limit from the catalog. luxe lowers it per request to what the window has left, so setting the real limit is safe; setting a lower one is how replies get cut off. |
| `name` | The name shown in the picker and statusline |
| `reasoning` | Whether it can think |
| `thinking` | The level a session starts at for this model |
| `thinking_level_map` | What each thinking level sends; see below |
| `modalities` | What it accepts: `["text"]` or `["text", "image"]` |
| `cost` | Prices per million tokens: `input`, `output`, `cache_read`, `cache_write`, and `cache_write_1h` for a write kept for an hour (twice `input` if unset) |
| `compat`, `sampling_params` | Override the provider's, key by key |

A model the catalog does not know gets a 128k context window and no output limit until you set
them. Anthropic endpoints require a limit, so there luxe asks for 128k. If the model allows less,
Anthropic's refusal names its real limit, and luxe retries with that and uses it for the rest of the
session.

## Thinking levels

`thinking_level_map` says what each of luxe's levels sends to the model. `null` marks a level the
model cannot do, removing it from `/thinking`; a level luxe needs falls back to the nearest one that
is left. `ultra` is the name for a provider's `max`.

```json title="~/.config/luxe/models.json"
{
  "models": {
    "lmstudio/qwen3.8-27b": {
      "reasoning": true,
      "thinking": "medium",
      "thinking_level_map": {
        "off": "none",
        "minimal": null,
        "low": "low",
        "medium": "medium",
        "high": "high",
        "xhigh": "high",
        "ultra": null
      }
    }
  }
}
```

A level you do not map sends its own name, and `off` sends `none`; `xhigh` and `ultra` are offered
only once mapped. Setting a map also marks the model as one that reasons.

An endpoint luxe does not know sends no thinking setting at all until its `compat` says how: for
most OpenAI-compatible servers, `"thinking_format": "openai"`, which sends `reasoning_effort` on
every request, `off` included. See [Request format](#request-format).

For Anthropic models, luxe uses adaptive thinking with an effort level where the model supports
it, and a token budget for older ones. If a model refuses how thinking was requested, luxe tries
the other way once and remembers it for the session. `compat.force_adaptive_thinking` forces
adaptive thinking for an endpoint the catalog does not know.

## Request format

`compat` says how a request has to be shaped for an endpoint to accept it. On a provider it applies
to every model the provider serves; on a model, only the keys you write override the provider's.

```json title="~/.config/luxe/models.json"
{
  "providers": {
    "lmstudio": {
      "base_url": "http://127.0.0.1:1234/v1",
      "compat": { "supports_developer_role": true, "supports_reasoning_effort": true, "thinking_format": "openai" }
    }
  },
  "models": {
    "lmstudio/qwen3.6-27b-fable-fusion": {
      "compat": { "thinking_format": "chat_template", "supports_developer_role": false }
    }
  }
}
```

| Flag | Does |
| --- | --- |
| `supports_developer_role` | Send the system prompt as `developer` rather than `system` |
| `supports_reasoning_effort` | Allow `reasoning_effort` to be sent |
| `thinking_format` | How thinking is requested: `openai`, `qwen`, `deepseek`, `openrouter`, `together` or `chat_template` |
| `chat_template_kwargs` | What to send for `chat_template`; see [below](#chat-templates) |
| `max_tokens_field` | `max_tokens` or `max_completion_tokens` |
| `supports_usage_in_streaming` | Ask for token usage while streaming. On by default. |
| `requires_tool_result_name` | Name the tool on each tool result. gpt-oss needs this. |
| `cache_control_format` | How prompt caching is requested: `anthropic` or `openrouter` |
| `force_adaptive_thinking` | Use Anthropic's adaptive thinking |
| `supports_long_cache_retention` | Ask Anthropic to keep cached prompts for an hour rather than five minutes. Each cache write costs 2x input instead of 1.25x. Off by default, since not every Anthropic-compatible endpoint accepts it. |
| `stream_tool_arguments` | Stream a `write` call's file from Anthropic as it is written, so its card fills in live. On by default; turn off for an endpoint that rejects a tool's `eager_input_streaming` field. |
| `defer_tool_loading` | Load a tool the agent asks for with `activate_tools` where it asked, so the cached prompt stays cached. On by default. luxe stops on its own for an endpoint that refuses it; turn it off for one that accepts it and then misbehaves. Anthropic and Responses endpoints only. |
| `copilot_headers`, `openrouter_headers` | Send those services' identifying headers. With `copilot_headers`, each model also goes to the Copilot endpoint it takes. |
| `use_responses_api` | Use OpenAI's Responses API. Provider only; prefer `"api": "openai-responses"`. |

`anthropic-messages` endpoints read only `force_adaptive_thinking`, `cache_control_format`,
`supports_long_cache_retention`, `stream_tool_arguments`, `defer_tool_loading` and
`copilot_headers`. The rest are for
OpenAI-style endpoints. `luxe models show` lists any flag that differs from the
provider's.

## Samplers

A local model's card often gives a sampling recipe with settings the OpenAI format has no field for,
such as `top_k`, `min_p` and `repeat_penalty`. `sampling_params` is added to the request as written,
after luxe's own fields, so its values win:

```json title="~/.config/luxe/models.json"
{
  "models": {
    "lmstudio/qwen3.8-27b": {
      "sampling_params": { "temperature": 1.0, "top_p": 0.95, "top_k": 20, "min_p": 0.0 }
    }
  },
  "providers": {
    "lmstudio": { "sampling_params": { "min_p": 0.0 } }
  }
}
```

A provider's samplers apply to every model it serves, and a model's override them one key at a
time. Nothing is checked, so an unknown key goes to your server as is; `model`, `messages`, `input`,
`stream` and `tools` are refused. OpenAI-style endpoints only.

## Chat templates

A server that renders the model's chat template, such as `llama-server`, takes thinking settings as
`chat_template_kwargs`. Each template names them differently, so you describe them:

```json title="~/.config/luxe/models.json"
{
  "providers": {
    "llama-server": {
      "base_url": "http://127.0.0.1:8080/v1",
      "compat": {
        "thinking_format": "chat_template",
        "chat_template_kwargs": {
          "enable_thinking": { "$var": "thinking.enabled" },
          "reasoning_effort": { "$var": "thinking.level", "omit_when_off": true },
          "preserve_thinking": true
        }
      }
    }
  }
}
```

`thinking.enabled` becomes `true` or `false`, and `thinking.level` the value `thinking_level_map`
gives the current level. `omit_when_off` leaves the key out when thinking is off. Anything else is
sent as written. LM Studio ignores `chat_template_kwargs`; use `"thinking_format": "openai"` there.

## Model details

luxe takes a model's details from three places, each overriding the one before, value by value:

1. The models.dev catalog built into luxe, which works offline
2. A fresher copy, refreshed in the background into `~/.cache/luxe/models-dev.json`
3. Your `models.json`

```console
$ luxe models show anthropic/claude-opus-5
anthropic/claude-opus-5
  name              Claude Opus 5  [models.dev]
  context window    1.0M  [models.dev]
  max output        128k  [models.dev]
  reasoning         true  [models.dev]
  thinking levels   high, low, medium, ultra, xhigh  [models.dev]
  thinking control  effort (adaptive thinking), never off  [models.dev]
  modalities        text, image  [models.dev]
  cost /Mtok        $5.00/$25.00  [models.dev]
```

Each value names where it came from: `snapshot` for the catalog built into luxe, `models.dev` for
the refreshed copy, `config` for your `models.json`, and `default` where none of them said.

| Variable | Does |
| --- | --- |
| `LUXE_MODELS_DEV_REFRESH=0` | Never refresh; the built-in catalog still answers |
| `LUXE_MODELS_DEV_URL` | Refresh from a mirror |
| `LUXE_MODELS_DEV_PATH` | Keep the catalog in this file instead. The refresh writes there too, so pair it with `LUXE_MODELS_DEV_REFRESH=0` to pin a file of your own. |
| `LUXE_MODELS_DEV_TTL` | Seconds between refreshes. 21600 by default. |

## Microsoft Foundry

### Keyless sign-in with Microsoft Entra ID

If your Foundry resource has key sign-in turned off, give luxe a command that prints a token:

```json title="~/.config/luxe/models.json"
{
  "providers": {
    "foundry-anthropic": {
      "auth": "bearer",
      "api_key": "!az account get-access-token --scope https://ai.azure.com/.default --query accessToken -o tsv"
    },
    "foundry-openai": {
      "api_key": "!az account get-access-token --scope https://ai.azure.com/.default --query accessToken -o tsv"
    }
  }
}
```

Run `az login` first. Only the Claude provider needs `auth`; the others already send a bearer
token. The command runs for each request and nothing is kept, so a token from `az`, which lasts
about an hour, is replaced as it expires and a long session carries on.

### Choose which deployments luxe offers

luxe records what a resource has deployed in each provider's `served_models`, and the model behind
any deployment named something else in `deployments`, which is where it looks up the context window
and price. If your key cannot list deployments, or you want to choose the list yourself, turn
discovery off and write both:

```json title="~/.config/luxe/models.json"
{
  "providers": {
    "foundry-anthropic": {
      "discover_deployments": false,
      "served_models": ["sonnet-prod", "claude-opus-5"],
      "deployments": { "sonnet-prod": "claude-sonnet-5" }
    }
  }
}
```

`discover_deployments: false` on any of the three providers stops luxe asking the resource at all.

### Thinking on partner models

`foundry-completions` sends no thinking setting by default, because its models disagree on how to
take one. Turn it on per model:

```json title="~/.config/luxe/models.json"
{
  "models": {
    "foundry-completions/deepseek-v4-flash": {
      "compat": { "supports_reasoning_effort": true },
      "thinking_level_map": { "high": "high", "ultra": "max" }
    }
  }
}
```

Use `reasoning_effort` only: Foundry refuses DeepSeek's own `thinking` format. Foundry does not
report these models' thinking tokens, so `/cost` counts them as output.

A Foundry deployment can have a different context window and price from the vendor's own API.
luxe uses Azure's figures for Foundry models.

## OpenRouter

- luxe asks OpenRouter to cache prompts for Claude models, which it otherwise does not, and which
  makes later turns of a session far cheaper. `"cache_control_format": null` in the provider's
  `compat` stops it.
- Some models on OpenRouter cannot turn thinking off. For those, luxe leaves the setting out and
  the model thinks at its own default. `luxe models show openrouter/<id>` says `never off`.
- luxe identifies itself to OpenRouter with its `HTTP-Referer` and `X-Title` headers, which carry
  nothing about you. `"openrouter_headers": false` in `compat` drops them.
