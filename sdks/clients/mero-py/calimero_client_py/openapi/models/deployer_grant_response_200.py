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
    from ..models.deployer_grant import DeployerGrant


T = TypeVar("T", bound="DeployerGrantResponse200")


@_attrs_define
class DeployerGrantResponse200:
    """
    Attributes:
        key (str):
        deployer (Union['DeployerType0', 'DeployerType1']): An account or an organization.
        grant (DeployerGrant): A grant to install where deployments are granted: until when (Unix seconds; absent: it
            does not end), when and by whom it was granted.
    """

    key: str
    deployer: Union["DeployerType0", "DeployerType1"]
    grant: "DeployerGrant"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1
        from ..models.deployer_grant import DeployerGrant

        key = self.key

        deployer: dict[str, Any]
        if isinstance(self.deployer, DeployerType0):
            deployer = self.deployer.to_dict()
        else:
            deployer = self.deployer.to_dict()

        grant = self.grant.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "key": key,
                "deployer": deployer,
                "grant": grant,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1
        from ..models.deployer_grant import DeployerGrant

        d = dict(src_dict)
        key = d.pop("key")

        def _parse_deployer(data: object) -> Union["DeployerType0", "DeployerType1"]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_deployer_type_0 = DeployerType0.from_dict(data)

                return componentsschemas_deployer_type_0
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_deployer_type_1 = DeployerType1.from_dict(data)

            return componentsschemas_deployer_type_1

        deployer = _parse_deployer(d.pop("deployer"))

        grant = DeployerGrant.from_dict(d.pop("grant"))

        deployer_grant_response_200 = cls(
            key=key,
            deployer=deployer,
            grant=grant,
        )

        deployer_grant_response_200.additional_properties = d
        return deployer_grant_response_200

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
