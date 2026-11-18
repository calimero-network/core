from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.package_ref import PackageRef


T = TypeVar("T", bound="Upgrade")


@_attrs_define
class Upgrade:
    """The last upgrade tried on an app, done or failed: `from` and `to` are bundle hashes, `by` the account (or
    `operator`), `run` the organization-wide run it was part of, `apiKey` the upgrade key that made it.

        Attributes:
            from_ (str):
            to (str):
            at (int):
            by (str):
            ok (bool):
            error (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
                it is).
            run (Union[Unset, str]):
            api_key (Union[Unset, str]):
            from_release (Union[Unset, PackageRef]): One service of one package release: `service` is absent for a single-
                service bundle, `compute` (absent: false) marks the service's compute component.
            to_release (Union[Unset, PackageRef]): One service of one package release: `service` is absent for a single-
                service bundle, `compute` (absent: false) marks the service's compute component.
    """

    from_: str
    to: str
    at: int
    by: str
    ok: bool
    error: Union[Unset, Any] = UNSET
    run: Union[Unset, str] = UNSET
    api_key: Union[Unset, str] = UNSET
    from_release: Union[Unset, "PackageRef"] = UNSET
    to_release: Union[Unset, "PackageRef"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package_ref import PackageRef

        from_ = self.from_

        to = self.to

        at = self.at

        by = self.by

        ok = self.ok

        error = self.error

        run = self.run

        api_key = self.api_key

        from_release: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.from_release, Unset):
            from_release = self.from_release.to_dict()

        to_release: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.to_release, Unset):
            to_release = self.to_release.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "from": from_,
                "to": to,
                "at": at,
                "by": by,
                "ok": ok,
            }
        )
        if error is not UNSET:
            field_dict["error"] = error
        if run is not UNSET:
            field_dict["run"] = run
        if api_key is not UNSET:
            field_dict["apiKey"] = api_key
        if from_release is not UNSET:
            field_dict["fromRelease"] = from_release
        if to_release is not UNSET:
            field_dict["toRelease"] = to_release

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package_ref import PackageRef

        d = dict(src_dict)
        from_ = d.pop("from")

        to = d.pop("to")

        at = d.pop("at")

        by = d.pop("by")

        ok = d.pop("ok")

        error = d.pop("error", UNSET)

        run = d.pop("run", UNSET)

        api_key = d.pop("apiKey", UNSET)

        _from_release = d.pop("fromRelease", UNSET)
        from_release: Union[Unset, PackageRef]
        if isinstance(_from_release, Unset):
            from_release = UNSET
        else:
            from_release = PackageRef.from_dict(_from_release)

        _to_release = d.pop("toRelease", UNSET)
        to_release: Union[Unset, PackageRef]
        if isinstance(_to_release, Unset):
            to_release = UNSET
        else:
            to_release = PackageRef.from_dict(_to_release)

        upgrade = cls(
            from_=from_,
            to=to,
            at=at,
            by=by,
            ok=ok,
            error=error,
            run=run,
            api_key=api_key,
            from_release=from_release,
            to_release=to_release,
        )

        upgrade.additional_properties = d
        return upgrade

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
