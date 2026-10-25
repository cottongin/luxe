---
title: "Models and providers"
summary: "Sign in to a provider, choose a model and thinking level, and run local models."
order: 9
group: "Using luxe"
---

## Sign in

Open **Configuration** (on the start screen, or `/config`) and choose how to connect. When a
provider connects, the model picker opens on its models; `tab` shows every provider's.

| Option | For |
| --- | --- |
| **Sign in to a subscription** | ChatGPT Plus or Pro, GitHub Copilot, or OpenRouter (which signs you in and creates a key) |
| **Add an API key** | Any provider below |
| **Connect a local or custom model** | LM Studio, Ollama, Azure, or any OpenAI-compatible endpoint |
| **Edit configuration files** | `config.json`, `models.json`, `auth.json` and `keybindings.json`, in `$EDITOR` |

From the command line:

```bash
luxe auth login                          # choose a provider from a list
luxe auth login anthropic                # or name one
luxe auth login openai --stdin < key.txt # read the key from a file or a pipe
luxe auth list                           # every provider, and how each is configured
luxe auth status                         # credential type, where it came from, when it expires
luxe auth logout openai                  # remove a stored credential
```

In a session, `/login`, `/logout` and `/auth` do the same.

Signing in checks the key with one request for the provider's model list, and gives up on an
endpoint that stops answering after `LUXE_STREAM_START_TIMEOUT` seconds. Typing an endpoint that is
plain `http://` and not on this machine works, with a warning that the key will cross the network
unencrypted.

### Providers

| Provider | ID | Environment variable |
| --- | --- | --- |
| OpenAI | `openai` | `OPENAI_API_KEY` |
| ChatGPT Plus or Pro | `openai-codex` | subscription only |
| Anthropic | `anthropic` | `ANTHROPIC_API_KEY` |
| GitHub Copilot | `github-copilot` | `COPILOT_GITHUB_TOKEN` |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` |
| Google Gemini | `google` | `GEMINI_API_KEY` or `GOOGLE_API_KEY` |
| xAI | `xai` | `XAI_API_KEY` |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY` |
| Groq | `groq` | `GROQ_API_KEY` |
| Cerebras | `cerebras` | `CEREBRAS_API_KEY` |
| Mistral | `mistral` | `MISTRAL_API_KEY` |
| Alibaba DashScope | `dashscope` | `DASHSCOPE_API_KEY` |
| Z.AI | `zai` | `ZAI_API_KEY` |
| Moonshot AI | `moonshotai` | `MOONSHOT_API_KEY` |
| Together AI | `togetherai` | `TOGETHER_API_KEY` |
| Fireworks AI | `fireworks` | `FIREWORKS_API_KEY` |
| NVIDIA NIM | `nvidia` | `NVIDIA_API_KEY` |
| Vercel AI Gateway | `vercel` | `AI_GATEWAY_API_KEY` |
| Microsoft Foundry | `foundry-openai`, `foundry-anthropic`, `foundry-completions` | `FOUNDRY_API_KEY` |
| Azure OpenAI | `azure` | `AZURE_OPENAI_API_KEY` |
| LM Studio | `lmstudio` | none |
| Ollama | `ollama` | none |
| Any OpenAI-compatible endpoint | `custom` | |

A provider's own variable works without signing in; when you have signed in too, the sign-in is
used. Anthropic and Google take API keys only: their terms do not allow their subscriptions to be
used from other programs.

GitHub Copilot serves Claude through Anthropic's Messages API, its newer GPT, Grok and MAI models
only through its Responses API, and the rest through chat completions. Copilot says which model
takes which, and luxe sends each one where it belongs. Claude goes to Messages even though chat
completions would take it, because only Messages keeps its thinking, its effort level and the
prompt cache for the whole conversation. The models offered are your plan's own, kept current each
time the sign-in refreshes. Signing in turns on any model your plan offers that you have not been
asked about yet; one you turned off stays off.

### Where credentials are kept

Credentials are stored in `~/.config/luxe/auth.json` (`%APPDATA%\luxe\auth.json` on Windows),
readable only by you. Subscriptions refresh on their own. A key can be the key itself, an
environment variable, or a command whose output is the key, so a password manager works:

```json title="~/.config/luxe/auth.json"
{
  "openai": { "type": "api_key", "key": "sk-..." },
  "anthropic": { "type": "api_key", "key": "!op read op://vault/anthropic/credential" }
}
```

A command runs each time a request needs the key.

## Choose a model

`/model` opens the model picker on the models of the provider you're using. Type to search, and
press `tab` to see every provider's models. The session carries on with the new model, even across
providers, and luxe remembers your choice for next time, in this project and as your latest
anywhere ([which model a session starts on](agent.md#which-model-a-session-starts-on)).

To change who serves the model, press `←` in the picker, or click the provider in the dock. Choose
a provider that serves the model you're on and luxe switches to it there, keeping the model.
Choose any other and its models open for you to pick from. Clicking the model in the dock opens
the models.

From the command line, `--model` takes a model ID, or `provider/id` to pick the provider too:

```bash
luxe --model anthropic/claude-opus-5
```

To see what luxe knows about a model, or list what you can use:

```bash
luxe models show anthropic/claude-opus-5   # context window, output limit, thinking levels, price
luxe models list --provider anthropic      # every model; * is active, + is configured
luxe models refresh --force                # update the model catalog now
```

luxe ships with the [models.dev](https://models.dev) catalog built in and refreshes it in the
background, so a model released after your copy of luxe still has the right context window and
price.

## Thinking

`/thinking` sets how hard the model reasons before answering: **Off**, **Minimal**, **Low**,
**Medium**, **High**, **XHigh** or **Ultra**. Levels the model does not support are grayed out.
`--thinking high` sets it at launch.

luxe remembers the level per model, since `high` on a frontier model and on a small local one cost
very different amounts of time. To give a model its own starting level:

```json title="~/.config/luxe/models.json"
{
  "models": {
    "lmstudio/qwen3.8-27b": { "thinking": "medium" }
  }
}
```

The picker marks that level `default`. `/thinking` can still change it during a session.

## Subscriptions

Usage on a ChatGPT or Copilot subscription is counted in full but never added to what you spent.
`/cost` shows what it would have cost at API prices, as
`plan ≈$0.42 of usage over 12 calls, covered by your subscription`. On a
ChatGPT plan, the statusline shows how much of the plan is used, such as `12% weekly`, and `/cost`
shows when it resets. On a Copilot plan it shows the month's allowance the same way, such as `19%
monthly`. The dock's [USAGE section](interface.md#the-dock) shows the same windows as a bar, with the
time until each resets when the provider says.

luxe prices Copilot usage at Copilot's per-token rates, so the figure matches what Copilot reports
for each request. A Copilot plan billed by the token spends its monthly allowance at those rates.

A ChatGPT subscription serves its own set of models, different from OpenAI's API, and `/model`
lists exactly those.

## Local models

**Connect a local or custom model** asks for the endpoint (blank takes the default) and records it
in `~/.config/luxe/models.json`. No credential is needed.

| Server | Default endpoint |
| --- | --- |
| LM Studio | `http://127.0.0.1:1234/v1` |
| Ollama | `http://127.0.0.1:11434/v1` |

The picker lists the models your server has loaded, asked fresh each time it opens. Embedding and
reranking models are left out.

For reasoning controls, chat templates and sampling settings on a local server, see
[Custom providers](custom-providers.md).

## Microsoft Foundry

One Foundry resource and key serve three APIs, which luxe treats as three providers. Signing in to
one configures all three:

```bash
luxe auth login foundry
```

It asks for the resource name, its endpoint, or a deployment's **Target URI**, and the key. Then
choose a model with the provider that serves it:

| Provider | Serves | Example |
| --- | --- | --- |
| `foundry-openai` | OpenAI's models | `luxe --model foundry-openai/gpt-5.6-terra` |
| `foundry-anthropic` | Claude | `luxe --model foundry-anthropic/claude-sonnet-5` |
| `foundry-completions` | DeepSeek, Grok, Llama, Mistral, Phi and Cohere | `luxe --model foundry-completions/deepseek-v4-pro` |

`/model` lists what your resource has deployed, not every model Azure offers. luxe asks the
resource for its deployments when you sign in and again each time you open `/model`, and sorts them
onto the right provider by the model each one runs. `luxe models refresh` asks too. If a resource
cannot list its deployments, `/model` shows Azure's catalog marked `unverified`, and you can
[list them yourself](custom-providers.md#choose-which-deployments-luxe-offers).

Use a deployment's name as the model ID, whatever it is called. For keyless sign-in with Microsoft
Entra ID, see [Custom providers](custom-providers.md#keyless-sign-in-with-microsoft-entra-id).

## When a provider stalls

luxe waits 60 seconds for a provider to start answering, and 180 seconds between parts of a reply.
If a provider is overloaded, rate limited or unreachable before it has sent anything, luxe tries
again after 2, 4 and 8 seconds, showing `retrying (2/3)` above the composer. Once a reply has
started, a failure ends the turn rather than paying for the reply twice. A bad key or a refused
request is not retried.

| Variable | Default | Sets |
| --- | --- | --- |
| `LUXE_STREAM_START_TIMEOUT` | `60` | Seconds to wait for a reply to start. `0` waits forever. |
| `LUXE_STREAM_IDLE_TIMEOUT` | `180` | Seconds a reply may go quiet. `0` waits forever. |
| `LUXE_PROVIDER_RETRIES` | `3` | Tries after the first. `0` never retries. |
