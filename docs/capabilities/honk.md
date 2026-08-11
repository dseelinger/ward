# Auto-honk

Fires the discovery scanner when you arrive in a system, so you do not have to.

Nothing asks Ward to do this. There is no phrase for it and no tool the model
can call. It happens because the game wrote a line saying you arrived
somewhere, and Ward was reading.

## Switching it on

Off unless you turn it on. Automation that presses your fire key should be
something you chose, not a surprise on first run.

Add this to `data/settings.json`:

```json
{
  "auto honk": true
}
```

## What it needs from your bindings

Elite has no separate binding for the discovery scanner. It fires on primary
fire with the scanner selected, so auto-honk means pressing your fire key —
and Ward can only press a key, never a joystick button.

Ward reads your active preset at startup and prefers a keyboard binding
wherever it finds one, including as a secondary. This binding works, because
the secondary is on the keyboard even though the primary is on the stick:

```xml
<PrimaryFire>
    <Primary Device="4098BEA1" Key="Joy_9" />
    <Secondary Device="Keyboard" Key="Key_F" />
</PrimaryFire>
```

If firing is only on your stick, Ward says so rather than doing nothing:

```
WARN ward::act: cannot fire the scanner reason="Firing is bound to your
joystick, which Ward cannot press. Bind it to a key as well and Ward can use it."
```

Bind primary fire to a key as well — it does not stop your stick working — and
auto-honk starts working with no other change.

## Why pressing a fire key here is safe

Hardpoints are retracted coming out of a hyperspace jump, so primary fire has
nothing to fire. That is the entire reason this is safe, and it is why it fires
on arrival and on nothing else.

Ward does not aim and does not decide. It presses a key you were always going
to press, at the one moment it does what you wanted.

## When it fires, and when it does not

It fires on `FSDJump` and on `CarrierJump`, and it holds the key for 400
milliseconds.

The game mentions a system more than once around a jump, so Ward remembers
where it last honked and skips a repeat. Two honks are one honk and one noise
for nothing.

Arriving somewhere new fires it again. Docking, undocking, dropping out of
supercruise and everything else the journal reports do not.

You will see this in `data/logs/ward.log` when it works:

```
INFO ward::act: firing the scanner action="PrimaryFire"
```

<!-- default: auto honk = false -->
