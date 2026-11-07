# Anything leaving the local machine requires human approval

Arbor never pushes, opens a PR, or performs any other remote-writing action without explicit Human Operator approval at that moment, every time. Read-only remote calls (`git fetch`, reading a GitHub issue) and the model calls to OpenRouter are the only exceptions. Approval is never remembered or carried over between actions.
