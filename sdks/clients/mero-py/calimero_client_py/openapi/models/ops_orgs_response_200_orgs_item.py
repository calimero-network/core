from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

T = TypeVar("T", bound="OpsOrgsResponse200OrgsItem")


@_attrs_define
class OpsOrgsResponse200OrgsItem:
    """
    Attributes:
        org (str):
        approved_releases (Union[None, list[str]]):
    """

    org: str
    approved_releases: Union[None, list[str]]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        org = self.org

        approved_releases: Union[None, list[str]]
        if isinstance(self.approved_releases, list):
            approved_releases = self.approved_releases

        else:
            approved_releases = self.approved_releases

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "org": org,
                "approvedReleases": approved_releases,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        org = d.pop("org")

        def _parse_approved_releases(data: object) -> Union[None, list[str]]:
            if data is None:
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                approved_releases_type_0 = cast(list[str], data)

                return approved_releases_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, list[str]], data)

        approved_releases = _parse_approved_releases(d.pop("approvedReleases"))

        ops_orgs_response_200_orgs_item = cls(
            org=org,
            approved_releases=approved_releases,
        )

        ops_orgs_response_200_orgs_item.additional_properties = d
        return ops_orgs_response_200_orgs_item

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
