from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="ServiceRefused")


@_attrs_define
class ServiceRefused:
    """`details` of a `400 UNSUPPORTED_BUNDLE`, `400 BAD_BUNDLE` or `400 COMPUTE_ABI_MISMATCH` refusing one service of a
    release: the service, and `compute: true` when it was that service's compute component. Absent when the refusal is
    of a raw bundle.

        Attributes:
            service (str):
            compute (Union[Unset, bool]):
    """

    service: str
    compute: Union[Unset, bool] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        service = self.service

        compute = self.compute

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "service": service,
            }
        )
        if compute is not UNSET:
            field_dict["compute"] = compute

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        service = d.pop("service")

        compute = d.pop("compute", UNSET)

        service_refused = cls(
            service=service,
            compute=compute,
        )

        service_refused.additional_properties = d
        return service_refused

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
