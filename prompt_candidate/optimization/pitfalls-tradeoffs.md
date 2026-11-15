Optimization often makes things worse. Avoid these traps:

- Tuning code that rarely runs. Work on what users wait for, such as start-up, the main interaction and the slowest common request.
- Caches without a way to invalidate them. A fast wrong answer is worse than a slow right one.
- Parallelism that costs more than it saves. Small workloads often run faster on one thread.
- Winning the benchmark but not real use. Test with realistic sizes and access patterns.
- Small gains bought with unreadable code. Keep a change only if the gain is worth the complexity it adds.
