"""Validates a review JSON file against review-schema.json (the subset of JSON Schema it uses).

Usage: validate_review.py <schema.json> <review.json>
Exits non-zero and prints the first violation when the review does not match.
"""

import json
import sys

TYPES = {"object": dict, "array": list, "string": str, "integer": int}


def check(value, schema, where):
    kind = schema.get("type")
    if kind and (not isinstance(value, TYPES[kind]) or isinstance(value, bool)):
        sys.exit(f"{where}: expected {kind}")
    if "enum" in schema and value not in schema["enum"]:
        sys.exit(f"{where}: {value!r} not in {schema['enum']}")
    if "minimum" in schema and value < schema["minimum"]:
        sys.exit(f"{where}: below minimum {schema['minimum']}")
    if kind == "object":
        for name in schema.get("required", []):
            if name not in value:
                sys.exit(f"{where}: missing {name}")
        if schema.get("additionalProperties") is False:
            for name in value:
                if name not in schema["properties"]:
                    sys.exit(f"{where}: unexpected {name}")
        for name, sub in schema.get("properties", {}).items():
            if name in value:
                check(value[name], sub, f"{where}.{name}")
    if kind == "array":
        for index, item in enumerate(value):
            check(item, schema["items"], f"{where}[{index}]")


def main():
    schema_path, review_path = sys.argv[1:3]
    with open(schema_path, encoding="utf-8") as handle:
        schema = json.load(handle)
    with open(review_path, encoding="utf-8") as handle:
        review = json.load(handle)
    check(review, schema, "review")


main()
