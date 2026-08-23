This reference shows a complete DataFrame workflow. It keeps the explanation, executable source code, and option table together so that readers can compare the operation with its expected inputs and outputs.

## Filtering rows

Use a column expression to keep rows above a threshold, then select the columns that the next step needs. The code block is part of the explanation and must remain intact even though it contains little prose.

```
import polars as pl

df = pl.DataFrame({"name": ["Ada", "Grace"], "value": [12, 7]})
result = df.filter(pl.col("value") > 10).select(["name", "value"])
print(result)
```

## Options

| Option | Meaning |
| --- | --- |
| name | Output column name |
| value | Numeric filter value |
| strict | Whether conversion errors are reported |
