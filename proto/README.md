# ProPresenter protobuf bindings

The source snapshot in `source/propresenter19/` is the MIT-licensed, unofficial `Proto 19beta` schema from [greyshirtguy/ProPresenter7-Proto](https://github.com/greyshirtguy/ProPresenter7-Proto), upstream commit `1b63dda196eb7e079721a8a4a7e7773520cb5ad2`. Its README identifies ProPresenter build 318767123. It was validated locally against ProPresenter 19.0.1 build 318767361 and document version 10/build 26200.

Run `uv run python scripts/update_proto_bindings.py` to generate the minimal transitive Python binding set under `src/versebridge/presentation/generated/`.

These schemas are reverse-engineered, unofficial, and unsupported by Renewed Vision. Compatibility must be explicitly gated and proven with sanitized golden fixtures before enabling writes for another version.
