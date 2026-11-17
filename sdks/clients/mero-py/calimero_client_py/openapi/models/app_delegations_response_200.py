from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.app_delegations_response_200_delegations import (
        AppDelegationsResponse200Delegations,
    )


T = TypeVar("T", bound="AppDelegationsResponse200")


@_attrs_define
class AppDelegationsResponse200:
    """
    Attributes:
        source (str):
        delegations (AppDelegationsResponse200Delegations):
    """

    source: str
    delegations: "AppDelegationsResponse200Delegations"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_delegations_response_200_delegations import (
            AppDelegationsResponse200Delegations,
        )

        source = self.source

        delegations = self.delegations.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "source": source,
                "delegations": delegations,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_delegations_response_200_delegations import (
            AppDelegationsResponse200Delegations,
        )

        d = dict(src_dict)
        source = d.pop("source")

        delegations = AppDelegationsResponse200Delegations.from_dict(
            d.pop("delegations")
        )

        app_delegations_response_200 = cls(
            source=source,
            delegations=delegations,
        )

        app_delegations_response_200.additional_properties = d
        return app_delegations_response_200

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
