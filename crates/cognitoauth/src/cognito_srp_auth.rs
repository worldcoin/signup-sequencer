use std::collections::HashMap;

use aws_config::meta::region::RegionProviderChain;
use aws_config::retry::RetryConfig;
use aws_config::{BehaviorVersion, Region};
use aws_sdk_cognitoidentityprovider::operation::respond_to_auth_challenge::RespondToAuthChallengeOutput;
use aws_sdk_cognitoidentityprovider::types::{
    AuthFlowType, AuthenticationResultType, ChallengeNameType,
};
use aws_sdk_cognitoidentityprovider::Client;
use cognito_srp::SrpClient;

use crate::error::CognitoSrpAuthError;

pub struct CognitoAuthInput {
    pub client_id:     String,
    pub pool_id:       String,
    pub username:      String,
    pub password:      String,
    pub mfa:           Option<String>,
    pub client_secret: Option<String>, // not yet supported
}

async fn get_cognito_idp_client(pool_id: &str) -> Result<Client, CognitoSrpAuthError> {
    let region = pool_id.split('_').next().map(|x| x.to_string());

    let region_provider = RegionProviderChain::first_try(region.map(Region::new))
        .or_default_provider()
        .or_else(Region::new("us-east-1"));

    let shared_config = aws_config::defaults(BehaviorVersion::latest())
        .region(region_provider)
        .load()
        .await;

    let cognito_idp_config = aws_sdk_cognitoidentityprovider::config::Builder::from(&shared_config)
        .retry_config(RetryConfig::disabled())
        .build();

    let cognito_client = Client::from_conf(cognito_idp_config);

    Ok(cognito_client)
}

async fn process_mfa(
    client: &Client,
    input: &CognitoAuthInput,
    auth_challenge_res: RespondToAuthChallengeOutput,
) -> Result<Option<AuthenticationResultType>, CognitoSrpAuthError> {
    let mfa = input
        .mfa
        .clone()
        .ok_or(CognitoSrpAuthError::IllegalArgument(
            "missing mfa but it is required".to_string(),
        ))?;
    let mut mfa_challenge_res: HashMap<String, String> = HashMap::new();
    mfa_challenge_res.insert("USERNAME".to_string(), input.username.to_string());
    mfa_challenge_res.insert("SOFTWARE_TOKEN_MFA_CODE".to_string(), mfa.to_string());

    let auth_challenge_res = client
        .respond_to_auth_challenge()
        .set_challenge_responses(Some(mfa_challenge_res))
        .client_id(input.client_id.clone())
        .challenge_name(ChallengeNameType::SoftwareTokenMfa)
        .session(auth_challenge_res.session.unwrap())
        .send()
        .await;

    let auth_res = auth_challenge_res?;

    Ok(auth_res.authentication_result)
}

pub async fn auth(
    input: CognitoAuthInput,
) -> Result<Option<AuthenticationResultType>, CognitoSrpAuthError> {
    let cognito_client = get_cognito_idp_client(&input.pool_id).await?;
    let srp_client = SrpClient::new(
        &input.username,
        &input.password,
        &input.pool_id,
        &input.client_id,
        None,
    );

    let auth_init_res = cognito_client
        .initiate_auth()
        .auth_flow(AuthFlowType::UserSrpAuth)
        .client_id(input.client_id.clone())
        .set_auth_parameters(Some(srp_client.get_auth_params()?))
        .send()
        .await;

    let auth_init_out = auth_init_res?;
    if auth_init_out.challenge_name.is_none()
        || auth_init_out.challenge_name.clone().unwrap() != ChallengeNameType::PasswordVerifier
    {
        if let Some(cn) = auth_init_out.challenge_name {
            tracing::debug!("challenge_name is unexpected, got {:?}", cn);
        } else {
            tracing::debug!("No challenge found in init");
        }
        return Ok(None);
    }

    let challenge_params =
        auth_init_out
            .challenge_parameters
            .ok_or(CognitoSrpAuthError::IllegalArgument(
                "No challenge was returned for the client".to_string(),
            ))?;
    let challenge_responses = srp_client.process_challenge(challenge_params)?;

    let password_challenge_res = cognito_client
        .respond_to_auth_challenge()
        .set_challenge_responses(Some(challenge_responses))
        .client_id(input.client_id.clone())
        .challenge_name(ChallengeNameType::PasswordVerifier)
        .send()
        .await?;

    match password_challenge_res.challenge_name {
        Some(ChallengeNameType::SoftwareTokenMfa) | Some(ChallengeNameType::SmsMfa) => {
            process_mfa(&cognito_client, &input, password_challenge_res).await
        }
        Some(_) | None => Ok(password_challenge_res.authentication_result),
    }
}
