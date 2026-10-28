# A limit on the model's answer

Every request to the language model (clean-up and report) says how many
tokens the answer may take, its reasoning included: 8192
(`polish::MAX_ANSWER_TOKENS`), as `max_completion_tokens`, or
`max_tokens` for Mistral. An answer cut before its text fails with a
message that says so, not as "an empty report".

## What happened

With `openai/gpt-oss-20b` on Groq, the report of a 28-minute meeting
(about 5 200 tokens in) came back empty. Asked the same thing several
times, the model reasoned until the answer's limit 2 times in 4:
`finish_reason: "length"`, `content: ""`, 2 048 tokens of reasoning. The
request named no limit, so the provider's default applied, and for
gpt-oss on Groq that is 2 048. The runs that finished reasoned for 1 250
to 1 900 tokens; Qwen, which reasons less, was never cut.

With a limit of 8192 both runs finished. Groq's free tier allows 8 000
tokens a minute, and it took that limit on a 5 200-token prompt: it
counts the tokens used, not the limit asked for.

## The field's name

`max_completion_tokens` is the name OpenAI's reasoning models accept
(they refuse `max_tokens`); Groq and vLLM take it, and Ollama ignores a
field it does not know. Mistral documents only `max_tokens` and is
expected to refuse a field it does not know (not tried: no key), so for
`api.mistral.ai` the field is `max_tokens`.

A model whose answer cannot be that long answers 400, and the error
shows it.
