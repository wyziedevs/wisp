<script>let { data } = $props();</script>
<h1>List</h1>
<ul>{#each data.items as item}<li>{item}</li>{/each}</ul>
