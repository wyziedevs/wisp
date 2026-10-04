---
let start = 2;
---

<title>Islands</title>
<h1>Islands</h1>
<Island of="react:$lib/Counter.js#Counter" props={:{ start, label: 'Count' }}>Loading</Island>
<Island
  of="react:react-switch"
  client:visible
  props={:{ checked: on, onChange: (v) => (on = v), 'aria-label': 'Power' }} />
<output>{:on ? 'on' : 'off'}</output>

<script>
  let on = false
</script>
