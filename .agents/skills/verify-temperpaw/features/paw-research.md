# Research (web queries)

## Sub-features
WebQuery entity: governed web search/fetch as entity flows.

## How to get to it (user POV)
An agent files a WebQuery; results land on the entity.

## Driving it
Prereq: EXA_API_KEY in env at boot (seeds the exa_api_key secret; without it search RecordErrors 'missing exa_api_key secret', it does NOT fail boot). Create a WebQuery {QueryType:'search',Query:'..',Url:''}, dispatch Temper.ExecuteSearch?await_integration=true, poll /observe/entities/WebQuery/<id>/wait?statuses=Complete,Failed, read results back. Fetch: QueryType:'fetch', Url set, Temper.ExecuteFetch.

## Gotchas
web_fetch/web_search are criticality=app-required. States Created->Executing->Complete|Failed. A completed-query cache means a repeated identical search returns the prior entity and creates no new WebQuery.
