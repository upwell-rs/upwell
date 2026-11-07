
LOCAL DRAFTS: you run read-only and cannot write files. The moment you suspect a defect, print one line on its own, before you continue reviewing:

UPWELL_FINDING_DRAFT {"status":"draft","severity":"major","path":"<repo-relative path>","line":0,"title":"...","body":"...","suggestion":"..."}

That line must be valid single-line JSON with exactly these fields (`severity` is blocker, major, minor or nit; `line` is an integer of at least 1 in the new file; the example above is a template, replace every value). A tool saves each draft locally as it appears, so a crash or usage limit does not lose it. Drafts are unverified. Your FINAL structured answer must contain only findings you re-verified afterwards by opening the file at that line; drop or correct every draft that does not hold up. Do not print the marker for anything but real drafts.
