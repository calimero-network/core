from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="ReportAppPublisherType0")


@_attrs_define
class ReportAppPublisherType0:
    """
    Attributes:
        signer (str):
        org (Union[None, str]):
    """

    signer: str
    org: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        signer = self.signer

        org: Union[None, str]
        org = self.org

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "signer": signer,
                "org": org,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        signer = d.pop("signer")

        def _parse_org(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        org = _parse_org(d.pop("org"))

        report_app_publisher_type_0 = cls(
            signer=signer,
            org=org,
        )

        report_app_publisher_type_0.additional_properties = d
        return report_app_publisher_type_0

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
