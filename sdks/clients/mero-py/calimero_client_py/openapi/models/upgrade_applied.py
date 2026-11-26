from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.upgrade_applied_status import UpgradeAppliedStatus

T = TypeVar("T", bound="UpgradeApplied")


@_attrs_define
class UpgradeApplied:
    """What `POST /apps/{app}/upgrade` answers: the app log's entry, how it ended, the app's error (`null` when it ended
    `ok`) and whether the release ships a compute component.

        Attributes:
            index (int):
            status (UpgradeAppliedStatus):
            error (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            compute (bool):
    """

    index: int
    status: UpgradeAppliedStatus
    error: Any
    compute: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        index = self.index

        status = self.status.value

        error = self.error

        compute = self.compute

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "index": index,
                "status": status,
                "error": error,
                "compute": compute,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        index = d.pop("index")

        status = UpgradeAppliedStatus(d.pop("status"))

        error = d.pop("error")

        compute = d.pop("compute")

        upgrade_applied = cls(
            index=index,
            status=status,
            error=error,
            compute=compute,
        )

        upgrade_applied.additional_properties = d
        return upgrade_applied

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
