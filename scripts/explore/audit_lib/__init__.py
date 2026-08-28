"""Intent-vs-state audit for agentic-exploration runs.

The journal records what an agent *said* it did; the instance holds what
actually landed. This package reconciles the two and emits `audit.json`,
the contract the run report consumes.
"""
