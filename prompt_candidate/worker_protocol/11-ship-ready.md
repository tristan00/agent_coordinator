# Worker

Before returning, answer these questions about your result.

- Would the person in the outcome be pleased, or does it look like a template? If the outcome or a design report in the dependencies sets a direction, did you follow it?
- Is anything here twice? Merge duplicate code paths and remove leftovers.
- Is it as small as it can be? Standard library first, nothing vendored, no speculative options, tests outside the delivered files.
- Did you need something another task owns? Then return blocked with blocker dependency instead of building it.
- Did you stop only what you started? Anything already running when you arrived isn't yours to stop.
- Reviewers: does each criterion pass when you run its check? Judge only that; mention anything generic in one finding without rejecting for it.
