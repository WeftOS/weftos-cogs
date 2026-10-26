# Troubleshoot

> What each failure looks like and what to do.

## The reply is "Sorry, I could not reach my language model."

`last_error` says why.

| `last_error` starts with | Meaning | Do this |
|---|---|---|
| `unavailable` | nobody serves `svc.infer/<service>` | place `npu-infer`; check the **Inference service** name |
| `timeout` | the queue or a model load took longer than **Call timeout** | raise it, or use a model that is already resident |
| `service_error` | the service refused the request, for example an unknown model | the message lists the models it serves |
| `unauthorized` | the cog was placed without its `svc.infer` input | place the package again |
| `the model's answer was empty` | only reasoning text came back | raise **Answer length** |

## No reply at all

`commands` is zero: `wake-word` publishes nothing. Check **Command source** against its **Name**.

## Answers ignore what was said before

**Remembered turns** is 0, or the earlier turns failed (a failed turn is not remembered).
