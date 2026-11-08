"""Validates a JSON file against one of the review schemas (the subset of JSON Schema they use).

Usage: validate_review.py <schema.json> <answer.json>
Exits non-zero and prints the first violation when the answer does not match. `violation` is the
importable form verify_drafts.py uses.
"""

import json
import sys

TYPES = {"object": dict, "array": list, "string": str, "integer": int}


def violation(value, schema, where):
    """Returns the first schema violation of `value` as text, or None when it matches."""
    kind = schema.get("type")

    if kind and (not isinstance(value, TYPES[kind]) or isinstance(value, bool)):
        return f"{where}: expected {kind}"

    if "enum" in schema and value not in schema["enum"]:
        return f"{where}: {value!r} not in {schema['enum']}"

    if "minimum" in schema and value < schema["minimum"]:
        return f"{where}: below minimum {schema['minimum']}"

    if kind == "object":
        return object_violation(value, schema, where)

    if kind == "array":
        for index, item in enumerate(value):
            found = violation(item, schema["items"], f"{where}[{index}]")

            if found:
                return found

    return None


def object_violation(value, schema, where):
    """Returns the first missing, unexpected or invalid property of an object, or None."""
    properties = schema.get("properties", {})

    for name in schema.get("required", []):
        if name not in value:
            return f"{where}: missing {name}"

    if schema.get("additionalProperties") is False:
        for name in value:
            if name not in properties:
                return f"{where}: unexpected {name}"

    for name, sub in properties.items():
        found = violation(value[name], sub, f"{where}.{name}") if name in value else None

        if found:
            return found

    return None


def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def main():
    schema_path, answer_path = sys.argv[1:3]

    try:
        answer = load(answer_path)
    except (OSError, ValueError) as error:
        sys.exit(f"{answer_path}: {error}")

    found = violation(answer, load(schema_path), "answer")

    if found:
        sys.exit(found)


if __name__ == "__main__":
    main()
