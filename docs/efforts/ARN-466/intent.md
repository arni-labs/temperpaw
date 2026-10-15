# ARN-466 — Computer Sleep is real; Effort.Merge ships

Computer.Sleep was a bare state change. Tensorlake could stay up while
the row said Ready (we kept paying), or Tensorlake could suspend on its
own while the row still said Ready (the keepwarm cron never fired). The
180s idle rule lived in chat.

WorkCycle already spawned ReleaseRun on Complete. Effort.Merge only
authorized and went to Merged. MarkDeployVerified is from Deploying, so
the ship child could never report back.

## Expected end state

- Ready → Sleep at 180s idle. Sleep POSTs Tensorlake `/suspend`. Wake
  POSTs `/resume`. Exec against Sleeping resumes first.
- Temper Computer status matches the sandbox (running or suspended).
- Effort.ConfigureDeploy then Effort.Merge creates a TemperDeploy into
  Deploying. A kernel Effort pins TemperPaw and sets that image tag
  before Merge.
- Agents work and ship on `Computers('arni-big')`. The laptop is not
  that computer.
