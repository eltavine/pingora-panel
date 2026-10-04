# 0027: Alerts

Status: accepted.

## Context

Operators want to be told when the gateway misbehaves: when server errors
or latency rise, when traffic stops, when upstreams fail. The product
specification asks for simple thresholds, webhook notifications and a
reserved interface for email. Per [ADR 0022](0022-metrics-logs-and-traces.md),
only `observability-service` reads Prometheus.

Conventions exist for each part.
[Prometheus alerting rules](https://prometheus.io/docs/prometheus/latest/configuration/alerting_rules/)
define how a condition becomes pending, fires after holding for a while
and resolves. Receivers built for
[Alertmanager's webhook payload](https://prometheus.io/docs/alerting/latest/configuration/#webhook_config)
are widely deployed. [Standard Webhooks](https://www.standardwebhooks.com/)
defines how a sender signs a webhook and how a receiver verifies it.

## Decision

**Rules.** A rule compares one measure, read over the last five minutes,
with a threshold, above or below it:

- the share of requests answered with a 5xx status;
- the 95th percentile request latency, in seconds;
- the request rate, per second;
- the share of failed upstream attempts;
- the gateway's open client connections.

Request measures read every site, one site or one route of a site; the
upstream measure reads every upstream or one. A rule fires once its
condition has held for its pending period, from none to a day, like a
Prometheus rule's `for`. It has a severity, warning or critical, can be
disabled, and names the channels it notifies. Rules and channels live in
the service's schema; their changes, refused changes and every alert that
fires or resolves are events in its outbox, and so in the audit trail.

**Evaluation.** Every 30 seconds, the instance that holds the schema's
alert lock, a PostgreSQL advisory lock, evaluates every enabled rule with
fixed PromQL over the metrics of ADR 0022; rules never carry PromQL. A
measure with no data does not meet its condition, except the request rate,
which reads no requests as zero. A rule whose condition holds becomes
pending, then firing once its pending period has passed, and resolves at
the first evaluation where the condition does not hold. A rule that cannot
be read keeps its state and reports why, so an unreachable Prometheus
neither fires nor resolves alerts.

**Notifications.** Firing and resolving queue a notification for each of
the rule's channels in the transaction that changes its state. Senders
claim queued notifications with `FOR UPDATE SKIP LOCKED`, so instances
share the work and none is sent twice at once, and retry failures after
growing delays, from 10 seconds to an hour, for a day. Notifications are
kept for seven days after they are delivered or abandoned.

A webhook channel receives a `POST` of Alertmanager's webhook payload,
version 4, so receivers written for Alertmanager accept it. Each is signed
as Standard Webhooks specify: `webhook-id` is the notification's ID, the
same on every attempt, with `webhook-timestamp` and a `webhook-signature`
of HMAC-SHA256 under the channel's `whsec_` secret. The service generates
the secret, shows it only when the channel is created or its secret is
replaced, and keeps it sealed with the deployment's master keys. A webhook
URL is `http` or `https`; redirects are not followed. A 2xx answer
delivers a notification; 408, 429, 5xx answers and network failures are
retried; any other answer abandons it. A test sends one notification at
once and reports how the receiver answered.

**Email** channels are reserved: the contract names them, and creating one
is refused as unsupported until the panel can send mail.

**Access.** `alerts.read` lists rules with their state, channels and
notifications; `alerts.manage` changes rules and channels and sends tests.

## Alternatives

- Prometheus alerting rules with Alertmanager are the usual pair, but both
  read their rules and receivers from files. The panel would render files
  into volumes shared with other containers and reload them over HTTP, so
  a change could not commit with its audit event, and Alertmanager does not
  sign webhooks. Its payload is kept, so its receivers still work.
- Grafana alerting would add a product to run, secure and upgrade for a
  handful of fixed measures.
- Alerting on log queries through Loki's ruler would read requests from
  logs that can be sampled, turned off per site or deleted.

## Consequences

- Alerts follow the measures of ADR 0022; a new measure is a contract
  change with its own fixed query.
- Evaluation needs PostgreSQL and Prometheus; while either is down, alerts
  keep their state, and notifications wait in the queue.
- Receivers verify notifications with any Standard Webhooks library and
  can process them as they process Alertmanager's.
