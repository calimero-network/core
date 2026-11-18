from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.deployer_type_0 import DeployerType0
    from ..models.deployer_type_1 import DeployerType1


T = TypeVar("T", bound="DeployerListResponse200GrantsItem")


@_attrs_define
class DeployerListResponse200GrantsItem:
    """
    Attributes:
        key (str):
        deployer (Union['DeployerType0', 'DeployerType1', None]):
        until (Union[None, str]):
        until_secs (Union[None, int]):
        granted_at (str):
        granted_by (str):
    """

    key: str
    deployer: Union["DeployerType0", "DeployerType1", None]
    until: Union[None, str]
    until_secs: Union[None, int]
    granted_at: str
    granted_by: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1

        key = self.key

        deployer: Union[None, dict[str, Any]]
        if isinstance(self.deployer, DeployerType0):
            deployer = self.deployer.to_dict()
        elif isinstance(self.deployer, DeployerType1):
            deployer = self.deployer.to_dict()
        else:
            deployer = self.deployer

        until: Union[None, str]
        until = self.until

        until_secs: Union[None, int]
        until_secs = self.until_secs

        granted_at = self.granted_at

        granted_by = self.granted_by

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "key": key,
                "deployer": deployer,
                "until": until,
                "untilSecs": until_secs,
                "grantedAt": granted_at,
                "grantedBy": granted_by,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1

        d = dict(src_dict)
        key = d.pop("key")

        def _parse_deployer(
            data: object,
        ) -> Union["DeployerType0", "DeployerType1", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_deployer_type_0 = DeployerType0.from_dict(data)

                return componentsschemas_deployer_type_0
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_deployer_type_1 = DeployerType1.from_dict(data)

                return componentsschemas_deployer_type_1
            except:  # noqa: E722
                pass
            return cast(Union["DeployerType0", "DeployerType1", None], data)

        deployer = _parse_deployer(d.pop("deployer"))

        def _parse_until(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        until = _parse_until(d.pop("until"))

        def _parse_until_secs(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        until_secs = _parse_until_secs(d.pop("untilSecs"))

        granted_at = d.pop("grantedAt")

        granted_by = d.pop("grantedBy")

        deployer_list_response_200_grants_item = cls(
            key=key,
            deployer=deployer,
            until=until,
            until_secs=until_secs,
            granted_at=granted_at,
            granted_by=granted_by,
        )

        deployer_list_response_200_grants_item.additional_properties = d
        return deployer_list_response_200_grants_item

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
